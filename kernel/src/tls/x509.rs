//! Minimal X.509 Certificate Parser
//!
//! Extracts only what TLS 1.3 needs: subject, issuer, public key, validity.
//! Zero-copy: all references point into the original DER bytes.

use super::asn1::{self, TAG_SEQUENCE, TAG_SET,
                   TAG_BIT_STRING, TAG_INTEGER,
                   TAG_OID, TAG_CONTEXT_0, TAG_UTC_TIME, TAG_GENERALIZED_TIME,
                   TAG_PRINTABLE_STRING, TAG_UTF8_STRING, TAG_IA5_STRING};

/// Parsed X.509 certificate (references into DER bytes)
#[allow(dead_code)]
pub struct X509Cert<'a> {
    /// The TBS (To Be Signed) certificate bytes (for signature verification)
    pub tbs_raw: &'a [u8],
    /// Issuer Common Name
    pub issuer_cn: &'a [u8],
    /// Subject Common Name
    pub subject_cn: &'a [u8],
    /// RSA modulus (big-endian)
    pub rsa_modulus: &'a [u8],
    /// RSA public exponent (big-endian)
    pub rsa_exponent: &'a [u8],
    /// Signature algorithm OID
    pub sig_algo_oid: &'a [u8],
    /// Signature bytes
    pub signature: &'a [u8],
    /// Validity: not-before (raw ASN.1 time string)
    pub not_before: &'a [u8],
    /// Validity: not-after (raw ASN.1 time string)
    pub not_after: &'a [u8],
    /// Is this a CA certificate?
    pub is_ca: bool,
}

// OID: 2.5.4.3 (commonName)
const OID_CN: &[u8] = &[0x55, 0x04, 0x03];

// OID: 1.2.840.113549.1.1.1 (rsaEncryption)
const OID_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];

// OID: 2.5.29.19 (basicConstraints)
const OID_BASIC_CONSTRAINTS: &[u8] = &[0x55, 0x1D, 0x13];

/// Parse a DER-encoded X.509 certificate.
pub fn parse_x509(der: &[u8]) -> Option<X509Cert<'_>> {
    // Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signatureValue }
    let (cert_seq, _) = asn1::parse_tlv(der)?;
    if cert_seq.tag != TAG_SEQUENCE { return None; }

    let mut items = asn1::parse_sequence_contents(cert_seq.value);

    // TBSCertificate
    let tbs_tlv = items.next()?;
    if tbs_tlv.tag != TAG_SEQUENCE { return None; }

    // We need the raw TBS bytes including tag+length for signature verification
    let tbs_raw = &der[cert_seq.value.as_ptr() as usize - der.as_ptr() as usize..];
    let (tbs_check, _) = asn1::parse_tlv(tbs_raw)?;
    let tbs_end = tbs_check.value.as_ptr() as usize + tbs_check.value.len() - der.as_ptr() as usize;
    let tbs_raw = &der[cert_seq.value.as_ptr() as usize - der.as_ptr() as usize..tbs_end];

    // Signature Algorithm
    let sig_algo_tlv = items.next()?;
    let sig_algo_oid = extract_algo_oid(sig_algo_tlv.value)?;

    // Signature Value (BIT STRING)
    let sig_tlv = items.next()?;
    if sig_tlv.tag != TAG_BIT_STRING { return None; }
    // Skip the "unused bits" byte
    let signature = if sig_tlv.value.len() > 1 { &sig_tlv.value[1..] } else { sig_tlv.value };

    // Parse TBSCertificate fields
    let mut tbs_items = asn1::parse_sequence_contents(tbs_tlv.value);

    // Version (optional, context [0])
    let first = tbs_items.next()?;
    let _serial_tlv = if first.tag == TAG_CONTEXT_0 {
        tbs_items.next()? // serial is next
    } else {
        first // no version field, this IS the serial
    };

    // Skip: signature algorithm (within TBS)
    let _ = tbs_items.next()?;

    // Issuer
    let issuer_tlv = tbs_items.next()?;
    let issuer_cn = extract_cn(issuer_tlv.value);

    // Validity
    let validity_tlv = tbs_items.next()?;
    let (not_before, not_after) = parse_validity(validity_tlv.value)?;

    // Subject
    let subject_tlv = tbs_items.next()?;
    let subject_cn = extract_cn(subject_tlv.value);

    // SubjectPublicKeyInfo
    let spki_tlv = tbs_items.next()?;
    let (rsa_modulus, rsa_exponent) = parse_rsa_public_key(spki_tlv.value)?;

    // Check for BasicConstraints in extensions (optional)
    let mut is_ca = false;
    // Try to find extensions (context [3])
    for tlv in tbs_items {
        if tlv.tag == asn1::TAG_CONTEXT_3 {
            is_ca = check_basic_constraints_ca(tlv.value);
        }
    }

    Some(X509Cert {
        tbs_raw,
        issuer_cn,
        subject_cn,
        rsa_modulus,
        rsa_exponent,
        sig_algo_oid,
        signature,
        not_before,
        not_after,
        is_ca,
    })
}

fn extract_algo_oid(algo_seq: &[u8]) -> Option<&[u8]> {
    let mut items = asn1::parse_sequence_contents(algo_seq);
    let oid = items.next()?;
    if oid.tag == TAG_OID { Some(oid.value) } else { None }
}

fn extract_cn(name_data: &[u8]) -> &[u8] {
    // Name is a SEQUENCE of SETs of AttributeTypeAndValue
    for set_tlv in asn1::parse_sequence_contents(name_data) {
        if set_tlv.tag != TAG_SET { continue; }
        for atv in asn1::parse_sequence_contents(set_tlv.value) {
            if atv.tag != TAG_SEQUENCE { continue; }
            let mut parts = asn1::parse_sequence_contents(atv.value);
            if let Some(oid) = parts.next() {
                if asn1::oid_matches(&oid, OID_CN) {
                    if let Some(val) = parts.next() {
                        if val.tag == TAG_PRINTABLE_STRING
                            || val.tag == TAG_UTF8_STRING
                            || val.tag == TAG_IA5_STRING
                        {
                            return val.value;
                        }
                    }
                }
            }
        }
    }
    b""
}

fn parse_validity(data: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut items = asn1::parse_sequence_contents(data);
    let nb = items.next()?;
    let na = items.next()?;
    if (nb.tag == TAG_UTC_TIME || nb.tag == TAG_GENERALIZED_TIME)
        && (na.tag == TAG_UTC_TIME || na.tag == TAG_GENERALIZED_TIME)
    {
        Some((nb.value, na.value))
    } else {
        None
    }
}

fn parse_rsa_public_key(spki_data: &[u8]) -> Option<(&[u8], &[u8])> {
    let mut items = asn1::parse_sequence_contents(spki_data);

    // AlgorithmIdentifier
    let algo = items.next()?;
    if algo.tag != TAG_SEQUENCE { return None; }
    let algo_oid = extract_algo_oid(algo.value)?;
    if algo_oid != OID_RSA { return None; } // Only RSA supported

    // SubjectPublicKey (BIT STRING containing SEQUENCE { modulus, exponent })
    let pubkey_bits = items.next()?;
    if pubkey_bits.tag != TAG_BIT_STRING { return None; }
    if pubkey_bits.value.is_empty() { return None; }
    let pubkey_data = &pubkey_bits.value[1..]; // Skip unused-bits byte

    let (seq, _) = asn1::parse_tlv(pubkey_data)?;
    if seq.tag != TAG_SEQUENCE { return None; }

    let mut parts = asn1::parse_sequence_contents(seq.value);
    let modulus_tlv = parts.next()?;
    let exponent_tlv = parts.next()?;
    if modulus_tlv.tag != TAG_INTEGER || exponent_tlv.tag != TAG_INTEGER { return None; }

    // Strip leading zero byte from modulus if present (ASN.1 integer encoding)
    let modulus = if !modulus_tlv.value.is_empty() && modulus_tlv.value[0] == 0 {
        &modulus_tlv.value[1..]
    } else {
        modulus_tlv.value
    };

    let exponent = if !exponent_tlv.value.is_empty() && exponent_tlv.value[0] == 0 {
        &exponent_tlv.value[1..]
    } else {
        exponent_tlv.value
    };

    Some((modulus, exponent))
}

fn check_basic_constraints_ca(ext_data: &[u8]) -> bool {
    // Extensions is a SEQUENCE of Extension
    for ext in asn1::parse_sequence_contents(ext_data) {
        if ext.tag != TAG_SEQUENCE { continue; }
        let mut parts = asn1::parse_sequence_contents(ext.value);
        if let Some(oid) = parts.next() {
            if asn1::oid_matches(&oid, OID_BASIC_CONSTRAINTS) {
                // May have critical BOOLEAN, then OCTET STRING containing SEQUENCE
                for part in parts {
                    if part.tag == asn1::TAG_OCTET_STRING {
                        if let Some((seq, _)) = asn1::parse_tlv(part.value) {
                            if seq.tag == TAG_SEQUENCE {
                                // BasicConstraints ::= SEQUENCE { cA BOOLEAN DEFAULT FALSE, ... }
                                for field in asn1::parse_sequence_contents(seq.value) {
                                    if field.tag == 0x01 { // BOOLEAN
                                        return field.value.first().copied() == Some(0xFF);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}
