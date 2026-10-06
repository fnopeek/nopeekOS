//! Cryptographic primitives for nopeekOS
//!
//! - ChaCha20 stream cipher (RFC 7539)
//! - Poly1305 MAC (RFC 7539)
//! - ChaCha20-Poly1305 AEAD (RFC 8439)
//! - The disk key hierarchy (BLAKE3 key derivation from the data key)
//!
//! All implementations target no_std bare metal.

use alloc::vec::Vec;
use spin::Mutex;

// ============================================================
// Disk keys (held while unlocked)
// ============================================================
//
// The data encryption key (DEK) comes out of a keyslot (`keyslot`). Nothing
// uses it directly: each purpose gets its own key, derived with BLAKE3's KDF
// mode under a fixed context string, so a key for one purpose says nothing
// about another.
//
//   data    — per-object AES-256-GCM keys
//   address — keyed BLAKE3 over an object's bytes: its content address. A
//             plain hash would let anyone holding the disk test whether a
//             known or guessed content is stored, a short secret included.
//   vault   — reserved for secrets kept apart from files

struct DiskKeys {
    data: [u8; 32],
    address: [u8; 32],
}

static DISK_KEYS: Mutex<Option<DiskKeys>> = Mutex::new(None);

const CTX_DATA: &str = "nopeekOS npkfs data key v1";
const CTX_ADDRESS: &str = "nopeekOS npkfs address key v1";
#[allow(dead_code)]
const CTX_VAULT: &str = "nopeekOS vault key v1";

/// Unlock: derive the working keys from the DEK.
pub fn set_disk_key(dek: &[u8; 32]) {
    *DISK_KEYS.lock() = Some(DiskKeys {
        data: blake3::derive_key(CTX_DATA, dek),
        address: blake3::derive_key(CTX_ADDRESS, dek),
    });
}

/// Lock: forget every key derived from the DEK.
pub fn clear_disk_key() {
    *DISK_KEYS.lock() = None;
}

pub fn disk_unlocked() -> bool {
    DISK_KEYS.lock().is_some()
}

/// A hasher that yields content addresses, or None while locked.
pub fn address_hasher() -> Option<blake3::Hasher> {
    DISK_KEYS.lock().as_ref().map(|k| blake3::Hasher::new_keyed(&k.address))
}

/// The content address of `bytes`, or None while locked.
pub fn object_address(bytes: &[u8]) -> Option<[u8; 32]> {
    let mut h = address_hasher()?;
    h.update(bytes);
    Some(*h.finalize().as_bytes())
}

/// The AES-256-GCM key and nonce for the object at `address`, or None while
/// locked. Every distinct content has its own key, so the nonce taken from
/// the address is never reused under a key.
pub fn object_key(address: &[u8; 32]) -> Option<([u8; 32], [u8; 12])> {
    let keys = DISK_KEYS.lock();
    let k = keys.as_ref()?;
    let mut h = blake3::Hasher::new_keyed(&k.data);
    h.update(address);
    let key = *h.finalize().as_bytes();
    let mut nonce = [0u8; 12];
    nonce.copy_from_slice(&address[..12]);
    Some((key, nonce))
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
// Poly1305 MAC (RFC 7539)
// ============================================================
//
// Uses u128 accumulator with explicit 130-bit representation (u128 + u8).
// Product computed via 64-bit limb schoolbook multiply into u128 intermediates.

fn poly1305_mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {
    // Clamp r
    let mut r_bytes = [0u8; 16];
    r_bytes.copy_from_slice(&key[..16]);
    r_bytes[3] &= 0x0f;  r_bytes[7] &= 0x0f;
    r_bytes[11] &= 0x0f; r_bytes[15] &= 0x0f;
    r_bytes[4] &= 0xfc;  r_bytes[8] &= 0xfc;  r_bytes[12] &= 0xfc;

    let r = u128::from_le_bytes(r_bytes);
    let s = u128::from_le_bytes({
        let mut b = [0u8; 16]; b.copy_from_slice(&key[16..32]); b
    });

    // Accumulator: (acc_lo: u128, acc_hi: u8) = 130-bit number
    let mut acc_lo: u128 = 0;
    let mut acc_hi: u8 = 0;

    let mut i = 0;
    while i + 16 <= message.len() {
        let n = u128::from_le_bytes({
            let mut b = [0u8; 16]; b.copy_from_slice(&message[i..i+16]); b
        });
        // acc += n + 2^128 (hibit)
        let (sum, carry1) = acc_lo.overflowing_add(n);
        acc_lo = sum;
        if carry1 { acc_hi += 1; }
        acc_hi += 1; // Add the 2^128 hibit

        // acc = acc * r mod (2^130 - 5)
        poly1305_mulmod(&mut acc_lo, &mut acc_hi, r);
        i += 16;
    }

    if i < message.len() {
        let remaining = message.len() - i;
        let mut block = [0u8; 17];
        block[..remaining].copy_from_slice(&message[i..]);
        block[remaining] = 0x01;

        let n = u128::from_le_bytes({
            let mut b = [0u8; 16];
            let copy_len = (remaining + 1).min(16);
            b[..copy_len].copy_from_slice(&block[..copy_len]);
            b
        });
        let extra_hi = if remaining + 1 > 16 { 1u8 } else { 0u8 };

        let (sum, carry1) = acc_lo.overflowing_add(n);
        acc_lo = sum;
        if carry1 { acc_hi += 1; }
        acc_hi += extra_hi;

        poly1305_mulmod(&mut acc_lo, &mut acc_hi, r);
    }

    // Final reduction mod p
    poly1305_reduce(&mut acc_lo, &mut acc_hi);

    // tag = (acc + s) mod 2^128
    let tag = acc_lo.wrapping_add(s);
    tag.to_le_bytes()
}

/// Multiply 130-bit accumulator by 128-bit r, reduce mod 2^130-5
#[allow(unused_variables, unused_mut, unused_assignments)]
fn poly1305_mulmod(acc_lo: &mut u128, acc_hi: &mut u8, r: u128) {
    // Split into 64-bit limbs for multiplication
    let a0 = *acc_lo as u64;
    let a1 = (*acc_lo >> 64) as u64;
    let a2 = *acc_hi as u64; // 0-3

    let r0 = r as u64;
    let r1 = (r >> 64) as u64;

    // Schoolbook: (a0 + a1*2^64 + a2*2^128) * (r0 + r1*2^64)
    let m00 = a0 as u128 * r0 as u128;
    let m01 = a0 as u128 * r1 as u128;
    let m10 = a1 as u128 * r0 as u128;
    let m11 = a1 as u128 * r1 as u128;
    let m20 = a2 as u128 * r0 as u128;
    let m21 = a2 as u128 * r1 as u128;

    // Combine: result = d[0] + d[1]*2^64 + d[2]*2^128 + d[3]*2^192
    let d0 = m00;
    let d1 = m01 + m10;   // might overflow u128 but won't in practice (64*64+64*64 < 2^129)
    let d1_carry = if d1 < m01 { 1u128 } else { 0 };
    let d2 = m11 + m20 + (d1_carry << 64);
    let d3 = m21;

    // Assemble 256-bit number as 4 x 64-bit
    let mut w0 = d0 as u64;
    let mut w1 = (d0 >> 64) as u64;
    let mut w2: u64 = 0;
    let mut w3: u64 = 0;

    let (t, c) = (w1 as u128 + (d1 as u64) as u128).overflowing_add(0); // can't overflow u128
    let add1 = w1 as u128 + d1 as u64 as u128;
    w1 = add1 as u64;
    let c1 = (add1 >> 64) as u64;

    let add2 = (d1 >> 64) as u64 as u128 + d2 as u64 as u128 + c1 as u128;
    w2 = add2 as u64;
    let c2 = (add2 >> 64) as u64;

    let add3 = (d2 >> 64) as u64 as u128 + d3 as u64 as u128 + c2 as u128;
    w3 = add3 as u64;

    let w4 = (d3 >> 64) as u64 + (add3 >> 64) as u64;

    // 320-bit result in w0..w4
    // Reduce mod 2^130 - 5: low 130 bits + (above >> 130) * 5
    let lo = w0 as u128 | ((w1 as u128) << 64);
    let lo_130 = lo & ((1u128 << 127) - 1 + (1u128 << 127)); // all 128 bits
    let bits_128_129 = (w2 & 3) as u8;

    // above 130 = w2>>2 | w3<<62 | w4<<126
    let above = (w2 >> 2) as u128 | ((w3 as u128) << 62) | ((w4 as u128) << 126);

    // result = lo_128 + bits_128_129 * 2^128 + above * 5
    let mul5 = above.wrapping_mul(5);
    let (new_lo, c) = lo.overflowing_add(mul5);
    let mut new_hi = bits_128_129;
    if c { new_hi += 1; }

    // If new_hi >= 4, reduce again
    if new_hi >= 4 {
        let extra = (new_hi >> 2) as u128;
        new_hi &= 3;
        let (new_lo2, c2) = new_lo.overflowing_add(extra * 5);
        *acc_lo = new_lo2;
        *acc_hi = new_hi + if c2 { 1 } else { 0 };
    } else {
        *acc_lo = new_lo;
        *acc_hi = new_hi;
    }
}

/// Final reduction: ensure acc < 2^130 - 5
fn poly1305_reduce(acc_lo: &mut u128, acc_hi: &mut u8) {
    // If acc >= p, subtract p
    // p = 2^130 - 5
    // Test: acc + 5 >= 2^130?
    let (test, c) = acc_lo.overflowing_add(5);
    let test_hi = *acc_hi + if c { 1 } else { 0 };
    if test_hi >= 4 {
        // acc >= p, use the reduced value
        *acc_lo = test;
        *acc_hi = test_hi & 3;
        // If still >= p after one reduction, do again (shouldn't happen)
        if *acc_hi >= 4 { *acc_hi &= 3; }
    }
    // Now acc < p. Take bottom 128 bits for final output.
    // acc_hi has bits 128-129, but for the tag we only need bits 0-127
    // (the final tag computation is (h + s) mod 2^128)
}

// ============================================================
// ChaCha20-Poly1305 AEAD (RFC 8439)
// ============================================================

pub const TAG_SIZE: usize = 16;

// AES-256-GCM via the `aes-gcm` crate. The crate's `aes` backend
// auto-detects AES-NI at runtime via `cpufeatures` and falls back to
// constant-time soft AES on CPUs without it.
//
// File-system encryption uses AES-GCM; TLS keeps ChaCha20-Poly1305 for
// cipher-suite compatibility with peers that negotiate it.

use aes_gcm::aead::{Aead, AeadInOut, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce, Tag};

/// Encrypt `plaintext` with AES-256-GCM. Returns ciphertext || 16-byte tag.
/// Hardware-accelerated when AES-NI is present.
pub fn aead_encrypt_aes(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {
    let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key");
    let nonce = Nonce::from(*nonce);
    cipher.encrypt(&nonce, plaintext).unwrap_or_default()
}

/// Decrypt `buf` (ciphertext || 16-byte tag) in place. On success `buf`
/// is shrunk to plaintext-only and `Some(())` is returned; on tag
/// mismatch / short input, `buf` is left untouched and `None` is
/// returned. Saves one Vec alloc + one full-payload memcpy.
pub fn aead_decrypt_aes_in_place(
    key: &[u8; 32],
    nonce: &[u8; 12],
    buf: &mut Vec<u8>,
) -> Option<()> {
    if buf.len() < TAG_SIZE { return None; }
    let cipher = Aes256Gcm::new_from_slice(key).ok()?;
    let nonce = Nonce::from(*nonce);

    let ct_len = buf.len() - TAG_SIZE;
    let tag_bytes: [u8; TAG_SIZE] = buf[ct_len..].try_into().ok()?;
    let tag = Tag::from(tag_bytes);

    cipher
        .decrypt_inout_detached(&nonce, b"", (&mut buf[..ct_len]).into(), &tag)
        .ok()?;
    buf.truncate(ct_len);
    Some(())
}

/// Encrypt and authenticate with Additional Authenticated Data (AAD).
/// ChaCha20-Poly1305. Used by TLS record layer when the negotiated
/// cipher suite is `TLS_CHACHA20_POLY1305_SHA256`. File-system
/// encryption uses AES-GCM via [`aead_encrypt_aes`] instead.
pub fn aead_encrypt_aad(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let poly_block = chacha20_block(key, 0, nonce);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&poly_block[..32]);

    let mut ciphertext = plaintext.to_vec();
    chacha20_xor(key, nonce, 1, &mut ciphertext);

    let mac_input = build_mac_input(aad, &ciphertext);
    let tag = poly1305_mac(&poly_key, &mac_input);

    ciphertext.extend_from_slice(&tag);
    ciphertext
}

/// Decrypt and verify with AAD. ChaCha20-Poly1305 — TLS counterpart
/// to [`aead_encrypt_aad`]. Storage path uses [`aead_decrypt_aes`].
pub fn aead_decrypt_aad(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {
    if ciphertext_and_tag.len() < TAG_SIZE {
        return None;
    }

    let ct_len = ciphertext_and_tag.len() - TAG_SIZE;
    let ciphertext = &ciphertext_and_tag[..ct_len];
    let tag = &ciphertext_and_tag[ct_len..];

    let poly_block = chacha20_block(key, 0, nonce);
    let mut poly_key = [0u8; 32];
    poly_key.copy_from_slice(&poly_block[..32]);

    let mac_input = build_mac_input(aad, ciphertext);
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


