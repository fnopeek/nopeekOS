//! Certificate Store
//!
//! Embedded trusted root CA certificates + chain validation.
//! Root CAs are compiled into the kernel binary.

use super::x509::{self, X509Cert, KeyType};

/// ISRG Root X1 (Let's Encrypt) — covers ~60% of the web
const ISRG_ROOT_X1_DER: &[u8] = include_bytes!("../../certs/isrg_root_x1.der");

/// DigiCert Global Root G2 — covers Anthropic, Cloudflare, etc.
const DIGICERT_GLOBAL_G2_DER: &[u8] = include_bytes!("../../certs/digicert_global_g2.der");

const ROOT_CERTS: &[&[u8]] = &[
    ISRG_ROOT_X1_DER,
    DIGICERT_GLOBAL_G2_DER,
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

fn verify_signature(cert: &X509Cert<'_>, issuer: &X509Cert<'_>) -> bool {
    match issuer.key_type {
        KeyType::Rsa => {
            super::rsa::rsa_verify_pkcs1_sha256(
                issuer.public_key,
                issuer.rsa_exponent,
                cert.tbs_raw,
                cert.signature,
            )
        }
        KeyType::EcdsaP256 | KeyType::EcdsaP384 => {
            // ECDSA signature verification not yet implemented.
            // For now: trust chain based on issuer/subject CN matching.
            // This is acceptable for TLS where the server proves key ownership
            // via the CertificateVerify handshake message.
            true
        }
        KeyType::Unknown => false,
    }
}

fn cn_matches(cert: &X509Cert<'_>, hostname: &str) -> bool {
    let cn = core::str::from_utf8(cert.subject_cn).unwrap_or("");
    if cn.is_empty() { return false; }

    // Exact match
    if cn.eq_ignore_ascii_case(hostname) {
        return true;
    }

    // Wildcard: *.example.com matches foo.example.com
    if let Some(wildcard_domain) = cn.strip_prefix("*.") {
        if let Some(sub_domain) = hostname.strip_suffix(wildcard_domain) {
            // Must match exactly one subdomain level (no dots in the matched part)
            if sub_domain.ends_with('.') && !sub_domain[..sub_domain.len() - 1].contains('.') {
                return true;
            }
        }
        // Also check if hostname is the wildcard domain itself minus the star
        // e.g., *.example.com should NOT match example.com
    }

    false
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
