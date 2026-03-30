//! HMAC-SHA256 and HKDF — wrappers around `hmac` and `hkdf` crates (RFC 5869, audited)

use alloc::vec::Vec;
use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// HMAC-SHA256
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC key");
    mac.update(message);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// HKDF-Extract (RFC 5869 Section 2.2)
pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    let salt = if salt.is_empty() { &[0u8; 32] as &[u8] } else { salt };
    hmac_sha256(salt, ikm)
}

/// HKDF-Expand (RFC 5869 Section 2.3)
pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {
    let hk = hkdf::Hkdf::<Sha256>::from_prk(prk).expect("PRK length");
    let mut okm = alloc::vec![0u8; length];
    hk.expand(info, &mut okm).expect("HKDF expand");
    okm
}

/// HKDF-Expand-Label for TLS 1.3 (RFC 8446 Section 7.1)
pub fn hkdf_expand_label(
    secret: &[u8; 32],
    label: &[u8],
    context: &[u8],
    length: usize,
) -> Vec<u8> {
    let full_label_len = 6 + label.len();
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
