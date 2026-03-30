//! HMAC-SHA256 and HKDF (RFC 5869)
//!
//! TLS 1.3 key schedule uses HKDF-Extract and HKDF-Expand-Label.

use alloc::vec::Vec;
use super::sha256::{Sha256, sha256};

const BLOCK_SIZE: usize = 64;
const HASH_SIZE: usize = 32;

/// HMAC-SHA256
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    // If key > block size, hash it
    let key_block = if key.len() > BLOCK_SIZE {
        let h = sha256(key);
        let mut kb = [0u8; BLOCK_SIZE];
        kb[..HASH_SIZE].copy_from_slice(&h);
        kb
    } else {
        let mut kb = [0u8; BLOCK_SIZE];
        kb[..key.len()].copy_from_slice(key);
        kb
    };

    // ipad = key XOR 0x36, opad = key XOR 0x5c
    let mut ipad = [0x36u8; BLOCK_SIZE];
    let mut opad = [0x5cu8; BLOCK_SIZE];
    for i in 0..BLOCK_SIZE {
        ipad[i] ^= key_block[i];
        opad[i] ^= key_block[i];
    }

    // inner = SHA256(ipad || message)
    let mut inner = Sha256::new();
    inner.update(&ipad);
    inner.update(message);
    let inner_hash = inner.finalize();

    // outer = SHA256(opad || inner_hash)
    let mut outer = Sha256::new();
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// HKDF-Extract (RFC 5869 Section 2.2)
/// PRK = HMAC-Hash(salt, IKM)
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let salt = if salt.is_empty() { &[0u8; 32] as &[u8] } else { salt };
    hmac_sha256(salt, ikm)
}

/// HKDF-Expand (RFC 5869 Section 2.3)
/// OKM = T(1) || T(2) || ... where T(i) = HMAC-Hash(PRK, T(i-1) || info || i)
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {
    let mut okm = Vec::with_capacity(length);
    let mut t = Vec::new();
    let n = (length + HASH_SIZE - 1) / HASH_SIZE;

    for i in 1..=n {
        let mut input = Vec::new();
        input.extend_from_slice(&t);
        input.extend_from_slice(info);
        input.push(i as u8);

        let block = hmac_sha256(prk, &input);
        t = block.to_vec();

        let take = (length - okm.len()).min(HASH_SIZE);
        okm.extend_from_slice(&block[..take]);
    }
    okm
}

/// HKDF-Expand-Label for TLS 1.3 (RFC 8446 Section 7.1)
/// Derive-Secret uses this internally.
pub fn hkdf_expand_label(
    secret: &[u8; 32],
    label: &[u8],
    context: &[u8],
    length: usize,
) -> Vec<u8> {
    // HkdfLabel = length (2 bytes) || "tls13 " || label || context
    let full_label_len = 6 + label.len(); // "tls13 " prefix
    let mut info = Vec::new();
    info.push((length >> 8) as u8);
    info.push(length as u8);
    info.push(full_label_len as u8);
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(label);
    info.push(context.len() as u8);
    info.extend_from_slice(context);

    hkdf_expand(secret, &info, length)
}

/// Derive-Secret for TLS 1.3 key schedule
pub fn derive_secret(secret: &[u8; 32], label: &[u8], transcript_hash: &[u8; 32]) -> [u8; 32] {
    let expanded = hkdf_expand_label(secret, label, transcript_hash, 32);
    let mut result = [0u8; 32];
    result.copy_from_slice(&expanded);
    result
}
