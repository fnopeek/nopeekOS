//! RSA signature verify — thin wrapper around RustCrypto `rsa`.
//!
//! PKCS#1 v1.5 for certificate signatures, PSS for the TLS 1.3
//! CertificateVerify. SHA-256 / SHA-384 only; SHA-1 is rejected.

use rsa::{RsaPublicKey, BigUint};
use rsa::signature::Verifier;
use rsa::pkcs1v15::{Signature, VerifyingKey};
use sha2::{Sha256, Sha384};

fn build_pubkey(modulus: &[u8], exponent: &[u8]) -> Option<RsaPublicKey> {
    let n = BigUint::from_bytes_be(modulus);
    let e = BigUint::from_bytes_be(exponent);
    RsaPublicKey::new(n, e).ok()
}

pub fn rsa_verify_pkcs1_sha256(
    modulus: &[u8], exponent: &[u8], message: &[u8], signature: &[u8],
) -> bool {
    let Some(pk) = build_pubkey(modulus, exponent) else { return false };
    let Ok(sig) = Signature::try_from(signature) else { return false };
    VerifyingKey::<Sha256>::new(pk).verify(message, &sig).is_ok()
}

pub fn rsa_verify_pkcs1_sha384(
    modulus: &[u8], exponent: &[u8], message: &[u8], signature: &[u8],
) -> bool {
    let Some(pk) = build_pubkey(modulus, exponent) else { return false };
    let Ok(sig) = Signature::try_from(signature) else { return false };
    VerifyingKey::<Sha384>::new(pk).verify(message, &sig).is_ok()
}

/// RSASSA-PSS with MGF1 over the same hash and a salt as long as the
/// digest — the only form TLS 1.3 accepts (RFC 8446 §4.2.3).
pub fn rsa_verify_pss_sha256(
    modulus: &[u8], exponent: &[u8], message: &[u8], signature: &[u8],
) -> bool {
    let Some(pk) = build_pubkey(modulus, exponent) else { return false };
    let Ok(sig) = rsa::pss::Signature::try_from(signature) else { return false };
    rsa::pss::VerifyingKey::<Sha256>::new(pk).verify(message, &sig).is_ok()
}

pub fn rsa_verify_pss_sha384(
    modulus: &[u8], exponent: &[u8], message: &[u8], signature: &[u8],
) -> bool {
    let Some(pk) = build_pubkey(modulus, exponent) else { return false };
    let Ok(sig) = rsa::pss::Signature::try_from(signature) else { return false };
    rsa::pss::VerifyingKey::<Sha384>::new(pk).verify(message, &sig).is_ok()
}
