//! Certificate Store
//!
//! Embedded trusted root CA certificates + chain validation.
//! Root CAs are compiled into the kernel binary.

use super::x509::{self, X509Cert, KeyType};
use super::sha256;

/// ISRG Root X1 (Let's Encrypt) — covers ~60% of the web
const ISRG_ROOT_X1_DER: &[u8] = include_bytes!("../../certs/isrg_root_x1.der");

/// DigiCert Global Root G2 — covers Anthropic, Cloudflare, etc.
const DIGICERT_GLOBAL_G2_DER: &[u8] = include_bytes!("../../certs/digicert_global_g2.der");

/// AAA Certificate Services (Comodo/Sectigo) — covers Cloudflare default certs
const AAA_CERT_SERVICES_DER: &[u8] = include_bytes!("../../certs/aaa_certificate_services.der");

/// Google Trust Services Root R1 — covers Google services
const GTS_ROOT_R1_DER: &[u8] = include_bytes!("../../certs/gts_root_r1.der");

const ROOT_CERTS: &[&[u8]] = &[
    ISRG_ROOT_X1_DER,
    DIGICERT_GLOBAL_G2_DER,
    AAA_CERT_SERVICES_DER,
    GTS_ROOT_R1_DER,
];

/// Verify a certificate chain.
/// `chain` is ordered leaf-first: [leaf, intermediate, ...].
/// Returns Ok(()) if the chain validates to a trusted root.
pub fn verify_chain(chain: &[&[u8]], hostname: &str) -> Result<(), CertError> {
    if chain.is_empty() {
        return Err(CertError::EmptyChain);
    }

    // Parse leaf certificate
    let leaf = x509::parse_x509(chain[0]).ok_or(CertError::ParseError)?;

    // Verify hostname matches leaf CN
    if !cn_matches(&leaf, hostname) {
        return Err(CertError::HostnameMismatch);
    }

    // Build chain: verify each cert is signed by the next
    let mut current = leaf;
    for i in 1..chain.len() {
        let issuer = x509::parse_x509(chain[i]).ok_or(CertError::ParseError)?;

        // Verify signature: current cert signed by issuer's public key
        if !verify_signature(&current, &issuer) {
            return Err(CertError::SignatureInvalid);
        }

        // Issuer must be a CA
        if !issuer.is_ca && i < chain.len() - 1 {
            return Err(CertError::NotCA);
        }

        current = issuer;
    }

    // The last cert in chain must be signed by a trusted root
    for root_der in ROOT_CERTS {
        if let Some(root) = x509::parse_x509(root_der) {
            // Check if current cert's issuer matches root's subject
            if current.issuer_cn == root.subject_cn {
                if verify_signature(&current, &root) {
                    return Ok(());
                }
            }
            // Also check if the last cert IS a root (self-signed)
            if current.subject_cn == root.subject_cn {
                if verify_signature(&current, &root) {
                    return Ok(());
                }
            }
        }
    }

    Err(CertError::UntrustedRoot)
}

// Signature algorithm OIDs
// 1.2.840.10045.4.3.2 = ecdsa-with-SHA256
const OID_ECDSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
// 1.2.840.10045.4.3.3 = ecdsa-with-SHA384
const OID_ECDSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];
// 1.2.840.113549.1.1.11 = sha256WithRSAEncryption
const OID_RSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];
// 1.2.840.113549.1.1.5 = sha1WithRSAEncryption
const OID_RSA_SHA1: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x05];
// 1.2.840.113549.1.1.12 = sha384WithRSAEncryption
const OID_RSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0C];

fn verify_signature(cert: &X509Cert<'_>, issuer: &X509Cert<'_>) -> bool {
    let algo = cert.sig_algo_oid;

    if algo == OID_RSA_SHA256 {
        super::rsa::rsa_verify_pkcs1_sha256(
            issuer.public_key,
            issuer.rsa_exponent,
            cert.tbs_raw,
            cert.signature,
        )
    } else if algo == OID_RSA_SHA1 {
        super::rsa::rsa_verify_pkcs1_sha1(
            issuer.public_key,
            issuer.rsa_exponent,
            cert.tbs_raw,
            cert.signature,
        )
    } else if algo == OID_RSA_SHA384 {
        super::rsa::rsa_verify_pkcs1_sha384(
            issuer.public_key,
            issuer.rsa_exponent,
            cert.tbs_raw,
            cert.signature,
        )
    } else if algo == OID_ECDSA_SHA256 {
        match issuer.key_type {
            KeyType::EcdsaP256 => ecdsa_p256_verify_sha256(issuer.public_key, cert.tbs_raw, cert.signature),
            _ => false,
        }
    } else if algo == OID_ECDSA_SHA384 {
        match issuer.key_type {
            KeyType::EcdsaP384 => ecdsa_p384_verify_sha384(issuer.public_key, cert.tbs_raw, cert.signature),
            _ => false,
        }
    } else {
        false
    }
}

/// ECDSA P-256 verify with SHA-256 digest.
fn ecdsa_p256_verify_sha256(pubkey: &[u8], tbs: &[u8], signature: &[u8]) -> bool {
    use p256::ecdsa::{VerifyingKey, Signature as P256Sig};
    use p256::ecdsa::signature::hazmat::PrehashVerifier;
    use p256::EncodedPoint;

    let point = match EncodedPoint::from_bytes(pubkey) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let vk = match VerifyingKey::from_encoded_point(&point) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let sig = match P256Sig::from_der(signature) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let digest = sha256::sha256(tbs);
    vk.verify_prehash(&digest, &sig).is_ok()
}

/// ECDSA P-384 verify with SHA-384 digest.
fn ecdsa_p384_verify_sha384(pubkey: &[u8], tbs: &[u8], signature: &[u8]) -> bool {
    use p384::ecdsa::{VerifyingKey, Signature as P384Sig};
    use p384::ecdsa::signature::hazmat::PrehashVerifier;
    use p384::EncodedPoint;

    let point = match EncodedPoint::from_bytes(pubkey) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let vk = match VerifyingKey::from_encoded_point(&point) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let sig = match P384Sig::from_der(signature) {
        Ok(s) => s,
        Err(_) => return false,
    };

    let digest = sha256::sha384(tbs);
    vk.verify_prehash(&digest, &sig).is_ok()
}

/// Verify an ECDSA P-384 signature over raw data (hashes internally with SHA-384).
/// pubkey: 97-byte uncompressed SEC1 point.
/// data: the raw data that was signed.
/// signature: DER-encoded ECDSA signature.
pub fn verify_p384_sha384(pubkey: &[u8], data: &[u8], signature: &[u8]) -> bool {
    use p384::ecdsa::{VerifyingKey, Signature as P384Sig, signature::Verifier};
    use p384::EncodedPoint;

    let point = match EncodedPoint::from_bytes(pubkey) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let vk = match VerifyingKey::from_encoded_point(&point) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let sig = match P384Sig::from_der(signature) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Verifier trait hashes data internally with SHA-384, then verifies
    vk.verify(data, &sig).is_ok()
}

/// Verify an ECDSA P-384 signature over a prehashed message (SHA-256).
#[allow(dead_code)]
pub fn verify_p384_prehash(pubkey: &[u8], prehash: &[u8; 32], signature: &[u8]) -> bool {
    use p384::ecdsa::{VerifyingKey, Signature as P384Sig};
    use p384::ecdsa::signature::hazmat::PrehashVerifier;
    use p384::EncodedPoint;

    let point = match EncodedPoint::from_bytes(pubkey) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let vk = match VerifyingKey::from_encoded_point(&point) {
        Ok(k) => k,
        Err(_) => return false,
    };
    let sig = match P384Sig::from_der(signature) {
        Ok(s) => s,
        Err(_) => return false,
    };
    vk.verify_prehash(prehash, &sig).is_ok()
}

fn cn_matches(cert: &X509Cert<'_>, hostname: &str) -> bool {
    // Check CN first
    let cn = core::str::from_utf8(cert.subject_cn).unwrap_or("");
    if !cn.is_empty() && name_matches(cn, hostname) {
        return true;
    }

    // Check SANs in TBS raw bytes (OID 2.5.29.17 = subjectAltName)
    if let Some(sans) = extract_sans(cert.tbs_raw) {
        for san in SanIter::new(sans) {
            if let Ok(name) = core::str::from_utf8(san) {
                if name_matches(name, hostname) {
                    return true;
                }
            }
        }
    }

    false
}

/// Check if a certificate name (CN or SAN) matches the hostname.
fn name_matches(name: &str, hostname: &str) -> bool {
    if name.eq_ignore_ascii_case(hostname) {
        return true;
    }
    // Wildcard: *.example.com matches foo.example.com
    if let Some(wildcard_domain) = name.strip_prefix("*.") {
        if let Some(sub_domain) = hostname.strip_suffix(wildcard_domain) {
            if sub_domain.ends_with('.') && !sub_domain[..sub_domain.len() - 1].contains('.') {
                return true;
            }
        }
    }
    false
}

// OID 2.5.29.17 = subjectAltName
const OID_SAN: &[u8] = &[0x55, 0x1D, 0x11];

/// Search TBS bytes for the SAN extension and return the inner SEQUENCE bytes.
fn extract_sans(tbs: &[u8]) -> Option<&[u8]> {
    // Scan for OID_SAN pattern in DER bytes
    for i in 0..tbs.len().saturating_sub(OID_SAN.len() + 4) {
        if &tbs[i..i + OID_SAN.len()] == OID_SAN {
            // After OID, skip to the OCTET STRING containing the SAN SEQUENCE
            let mut pos = i + OID_SAN.len();
            // There may be a BOOLEAN (critical) before the OCTET STRING
            while pos < tbs.len() {
                let tag = tbs[pos];
                if tag == 0x04 { // OCTET STRING
                    pos += 1;
                    let (len, hdr) = der_len(&tbs[pos..])?;
                    pos += hdr;
                    if pos + len <= tbs.len() {
                        return Some(&tbs[pos..pos + len]);
                    }
                    return None;
                } else if tag == 0x01 { // BOOLEAN (critical flag)
                    pos += 1;
                    let (len, hdr) = der_len(&tbs[pos..])?;
                    pos += hdr + len;
                } else {
                    break;
                }
            }
        }
    }
    None
}

fn der_len(data: &[u8]) -> Option<(usize, usize)> {
    if data.is_empty() { return None; }
    if data[0] < 0x80 {
        Some((data[0] as usize, 1))
    } else if data[0] == 0x81 && data.len() > 1 {
        Some((data[1] as usize, 2))
    } else if data[0] == 0x82 && data.len() > 2 {
        Some((((data[1] as usize) << 8) | data[2] as usize, 3))
    } else {
        None
    }
}

/// Iterator over DNS names in a SAN extension (tag 0x82 = dNSName).
struct SanIter<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> SanIter<'a> {
    fn new(data: &'a [u8]) -> Self {
        // Skip outer SEQUENCE tag if present
        let mut pos = 0;
        if !data.is_empty() && data[0] == 0x30 {
            pos = 1;
            if let Some((_, hdr)) = der_len(&data[1..]) {
                pos += hdr;
            }
        }
        SanIter { data, pos }
    }
}

impl<'a> Iterator for SanIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        while self.pos < self.data.len() {
            let tag = self.data[self.pos];
            self.pos += 1;
            let (len, hdr) = der_len(&self.data[self.pos..])?;
            self.pos += hdr;
            let value = &self.data[self.pos..self.pos + len.min(self.data.len() - self.pos)];
            self.pos += len;

            // Tag 0x82 = context-specific [2] = dNSName
            if tag == 0x82 {
                return Some(value);
            }
        }
        None
    }
}

#[derive(Debug)]
pub enum CertError {
    EmptyChain,
    ParseError,
    HostnameMismatch,
    SignatureInvalid,
    NotCA,
    UntrustedRoot,
}

impl core::fmt::Display for CertError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            CertError::EmptyChain => write!(f, "empty certificate chain"),
            CertError::ParseError => write!(f, "certificate parse error"),
            CertError::HostnameMismatch => write!(f, "hostname mismatch"),
            CertError::SignatureInvalid => write!(f, "invalid signature"),
            CertError::NotCA => write!(f, "intermediate is not a CA"),
            CertError::UntrustedRoot => write!(f, "untrusted root CA"),
        }
    }
}
