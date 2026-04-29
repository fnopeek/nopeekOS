//! RSA PKCS#1 v1.5 signature verify — thin wrapper around RustCrypto `rsa`.
//!
//! SHA-256 / SHA-384 only. SHA-1 chains are rejected.

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
