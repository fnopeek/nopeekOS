//! Minimal X.509 Certificate Parser
//!
//! Extracts what TLS 1.3 chain validation needs: subject, issuer, public key,
//! validity, BasicConstraints (CA + pathLen), KeyUsage, ExtendedKeyUsage,
//! and a flag for unknown critical extensions.

use super::asn1::{self, TAG_SEQUENCE, TAG_SET,
                   TAG_BIT_STRING, TAG_INTEGER,
                   TAG_OID, TAG_CONTEXT_0, TAG_UTC_TIME, TAG_GENERALIZED_TIME,
                   TAG_PRINTABLE_STRING, TAG_UTF8_STRING, TAG_IA5_STRING,
                   TAG_OCTET_STRING};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyType {
    Rsa,
    EcdsaP256,
    EcdsaP384,
    Unknown,
}

// RFC 5280 §4.2.1.3 — KeyUsage bit positions.
pub const KU_DIGITAL_SIGNATURE: u16 = 1 << 0;
#[allow(dead_code)]
pub const KU_CONTENT_COMMITMENT: u16 = 1 << 1;
#[allow(dead_code)]
pub const KU_KEY_ENCIPHERMENT: u16 = 1 << 2;
#[allow(dead_code)]
pub const KU_DATA_ENCIPHERMENT: u16 = 1 << 3;
#[allow(dead_code)]
pub const KU_KEY_AGREEMENT: u16 = 1 << 4;
pub const KU_KEY_CERT_SIGN: u16 = 1 << 5;
#[allow(dead_code)]
pub const KU_CRL_SIGN: u16 = 1 << 6;

/// Parsed X.509 certificate (references into DER bytes)
#[allow(dead_code)]
pub struct X509Cert<'a> {
    pub tbs_raw: &'a [u8],
    pub issuer_cn: &'a [u8],
    pub subject_cn: &'a [u8],
    pub key_type: KeyType,
    /// RSA: modulus bytes. ECDSA: raw public key point (uncompressed).
    pub public_key: &'a [u8],
    /// RSA: exponent bytes. ECDSA: empty.
    pub rsa_exponent: &'a [u8],
    pub sig_algo_oid: &'a [u8],
    pub signature: &'a [u8],
    pub not_before: &'a [u8],
    pub not_after: &'a [u8],
    pub is_ca: bool,
    /// BasicConstraints pathLenConstraint, if present. None = unconstrained.
    pub path_len_constraint: Option<u32>,
    /// KeyUsage bits (KU_*). None = extension absent (no KU constraint per RFC 5280).
    pub key_usage: Option<u16>,
    /// True if EKU extension is present.
    pub eku_present: bool,
    /// True if EKU contains id-kp-serverAuth (1.3.6.1.5.5.7.3.1).
    pub eku_server_auth: bool,
    /// True if EKU contains anyExtendedKeyUsage (2.5.29.37.0).
    pub eku_any: bool,
    /// True if a critical extension was found that we don't know how to process.
    /// Per RFC 5280 §4.2: such certs MUST be rejected.
    pub unknown_critical_ext: bool,
}

// OID: 2.5.4.3 (commonName)
const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
// OID: 1.2.840.113549.1.1.1 (rsaEncryption)
const OID_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
// OID: 1.2.840.10045.2.1 (ecPublicKey)
const OID_EC: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
// OID: 1.2.840.10045.3.1.7 (P-256 / secp256r1)
const OID_P256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
// OID: 1.3.132.0.34 (P-384 / secp384r1)
const OID_P384: &[u8] = &[0x2B, 0x81, 0x04, 0x00, 0x22];

// Extension OIDs we recognise.
const OID_EXT_SAN:                &[u8] = &[0x55, 0x1D, 0x11]; // 2.5.29.17
const OID_EXT_BASIC_CONSTRAINTS:  &[u8] = &[0x55, 0x1D, 0x13]; // 2.5.29.19
const OID_EXT_KEY_USAGE:          &[u8] = &[0x55, 0x1D, 0x0F]; // 2.5.29.15
const OID_EXT_EXT_KEY_USAGE:      &[u8] = &[0x55, 0x1D, 0x25]; // 2.5.29.37

// EKU purpose OIDs.
const OID_KP_SERVER_AUTH: &[u8] = &[0x2B, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x01]; // 1.3.6.1.5.5.7.3.1
const OID_KP_ANY:         &[u8] = &[0x55, 0x1D, 0x25, 0x00];                          // 2.5.29.37.0

/// Parse a DER-encoded X.509 certificate.
pub fn parse_x509(der: &[u8]) -> Option<X509Cert<'_>> {
    let (cert_seq, _) = asn1::parse_tlv(der)?;
    if cert_seq.tag != TAG_SEQUENCE { return None; }

    let mut items = asn1::parse_sequence_contents(cert_seq.value);

    // TBSCertificate
    let tbs_tlv = items.next()?;
    if tbs_tlv.tag != TAG_SEQUENCE { return None; }

    // Raw TBS bytes (including tag+length) for signature verification
    let tbs_offset = cert_seq.value.as_ptr() as usize - der.as_ptr() as usize;
    let (tbs_check, _) = asn1::parse_tlv(&der[tbs_offset..])?;
    let tbs_end = tbs_check.value.as_ptr() as usize + tbs_check.value.len() - der.as_ptr() as usize;
    let tbs_raw = &der[tbs_offset..tbs_end];

    // Signature Algorithm
    let sig_algo_tlv = items.next()?;
    let sig_algo_oid = extract_algo_oid(sig_algo_tlv.value)?;

    // Signature Value (BIT STRING)
    let sig_tlv = items.next()?;
    if sig_tlv.tag != TAG_BIT_STRING { return None; }
    let signature = if sig_tlv.value.len() > 1 { &sig_tlv.value[1..] } else { sig_tlv.value };

    // Parse TBSCertificate fields
    let mut tbs_items = asn1::parse_sequence_contents(tbs_tlv.value);

    // Version (optional, context [0])
    let first = tbs_items.next()?;
    let _serial_tlv = if first.tag == TAG_CONTEXT_0 {
        tbs_items.next()?
    } else {
        first
    };

    // Signature algorithm (within TBS)
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
    let (key_type, public_key, rsa_exponent) = parse_public_key(spki_tlv.value)?;

    // Extensions (optional, context [3])
    let mut is_ca = false;
    let mut path_len_constraint = None;
    let mut key_usage = None;
    let mut eku_present = false;
    let mut eku_server_auth = false;
    let mut eku_any = false;
    let mut unknown_critical_ext = false;

    for tlv in tbs_items {
        if tlv.tag == asn1::TAG_CONTEXT_3 {
            parse_extensions(
                tlv.value,
                &mut is_ca,
                &mut path_len_constraint,
                &mut key_usage,
                &mut eku_present,
                &mut eku_server_auth,
                &mut eku_any,
                &mut unknown_critical_ext,
            );
        }
    }

    Some(X509Cert {
        tbs_raw, issuer_cn, subject_cn,
        key_type, public_key, rsa_exponent,
        sig_algo_oid, signature, not_before, not_after,
        is_ca, path_len_constraint,
        key_usage, eku_present, eku_server_auth, eku_any,
        unknown_critical_ext,
    })
}

fn extract_algo_oid(algo_seq: &[u8]) -> Option<&[u8]> {
    let mut items = asn1::parse_sequence_contents(algo_seq);
    let oid = items.next()?;
    if oid.tag == TAG_OID { Some(oid.value) } else { None }
}

fn extract_cn(name_data: &[u8]) -> &[u8] {
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

/// Parse SubjectPublicKeyInfo, supporting RSA and ECDSA keys.
fn parse_public_key(spki_data: &[u8]) -> Option<(KeyType, &[u8], &[u8])> {
    let mut items = asn1::parse_sequence_contents(spki_data);

    // AlgorithmIdentifier (SEQUENCE { OID, params })
    let algo = items.next()?;
    if algo.tag != TAG_SEQUENCE { return None; }

    let mut algo_parts = asn1::parse_sequence_contents(algo.value);
    let algo_oid = algo_parts.next()?;
    if algo_oid.tag != TAG_OID { return None; }

    // SubjectPublicKey (BIT STRING)
    let pubkey_bits = items.next()?;
    if pubkey_bits.tag != TAG_BIT_STRING { return None; }
    if pubkey_bits.value.is_empty() { return None; }
    let pubkey_data = &pubkey_bits.value[1..]; // Skip unused-bits byte

    if algo_oid.value == OID_RSA {
        // RSA: BIT STRING contains SEQUENCE { modulus INTEGER, exponent INTEGER }
        let (seq, _) = asn1::parse_tlv(pubkey_data)?;
        if seq.tag != TAG_SEQUENCE { return None; }
        let mut parts = asn1::parse_sequence_contents(seq.value);
        let modulus_tlv = parts.next()?;
        let exponent_tlv = parts.next()?;
        if modulus_tlv.tag != TAG_INTEGER || exponent_tlv.tag != TAG_INTEGER { return None; }

        let modulus = strip_leading_zero(modulus_tlv.value);
        let exponent = strip_leading_zero(exponent_tlv.value);
        Some((KeyType::Rsa, modulus, exponent))
    } else if algo_oid.value == OID_EC {
        // ECDSA: params contain the curve OID, BIT STRING is the raw point
        let curve = algo_parts.next();
        let key_type = match curve {
            Some(c) if c.tag == TAG_OID && c.value == OID_P256 => KeyType::EcdsaP256,
            Some(c) if c.tag == TAG_OID && c.value == OID_P384 => KeyType::EcdsaP384,
            _ => KeyType::Unknown,
        };
        Some((key_type, pubkey_data, &[]))
    } else {
        Some((KeyType::Unknown, pubkey_data, &[]))
    }
}

fn strip_leading_zero(data: &[u8]) -> &[u8] {
    if !data.is_empty() && data[0] == 0 { &data[1..] } else { data }
}

/// Walk the [3] EXPLICIT extensions wrapper. For each extension, dispatch on
/// OID; record an unknown-critical hit when the extnID is unrecognised AND
/// the critical flag is TRUE.
fn parse_extensions(
    ext_data: &[u8],
    is_ca: &mut bool,
    path_len: &mut Option<u32>,
    key_usage: &mut Option<u16>,
    eku_present: &mut bool,
    eku_server_auth: &mut bool,
    eku_any: &mut bool,
    unknown_critical: &mut bool,
) {
    let outer = match asn1::parse_tlv(ext_data) {
        Some((tlv, _)) if tlv.tag == TAG_SEQUENCE => tlv,
        _ => return,
    };

    for ext in asn1::parse_sequence_contents(outer.value) {
        if ext.tag != TAG_SEQUENCE { continue; }
        let mut parts = asn1::parse_sequence_contents(ext.value);

        let oid_tlv = match parts.next() {
            Some(t) if t.tag == TAG_OID => t,
            _ => continue,
        };

        // Optional BOOLEAN (critical), then mandatory OCTET STRING (extnValue).
        let mut critical = false;
        let mut value: &[u8] = &[];
        for p in parts {
            match p.tag {
                0x01 => critical = !p.value.is_empty() && p.value[0] != 0x00,
                t if t == TAG_OCTET_STRING => value = p.value,
                _ => {}
            }
        }

        let oid = oid_tlv.value;
        if oid == OID_EXT_BASIC_CONSTRAINTS {
            parse_basic_constraints(value, is_ca, path_len);
        } else if oid == OID_EXT_KEY_USAGE {
            *key_usage = parse_key_usage(value);
        } else if oid == OID_EXT_EXT_KEY_USAGE {
            *eku_present = true;
            parse_eku(value, eku_server_auth, eku_any);
        } else if oid == OID_EXT_SAN {
            // Recognised: handled separately by certstore via raw-TBS scan.
        } else if critical {
            *unknown_critical = true;
        }
    }
}

fn parse_basic_constraints(value: &[u8], is_ca: &mut bool, path_len: &mut Option<u32>) {
    let (seq, _) = match asn1::parse_tlv(value) {
        Some(t) => t,
        None => return,
    };
    if seq.tag != TAG_SEQUENCE { return; }
    for field in asn1::parse_sequence_contents(seq.value) {
        match field.tag {
            0x01 => {
                *is_ca = field.value.first().copied() == Some(0xFF);
            }
            TAG_INTEGER => {
                let mut n: u32 = 0;
                for &b in field.value {
                    n = (n << 8) | b as u32;
                    if n > 0xFFFF { return; } // sanity cap
                }
                *path_len = Some(n);
            }
            _ => {}
        }
    }
}

/// Decode KeyUsage BIT STRING into a u16 with KU_* bit positions.
fn parse_key_usage(value: &[u8]) -> Option<u16> {
    let (bs, _) = asn1::parse_tlv(value)?;
    if bs.tag != TAG_BIT_STRING || bs.value.len() < 2 { return None; }
    let unused_bits = bs.value[0] as u32;
    if unused_bits > 7 { return None; }
    // RFC 5280: bits are numbered from the MSB. Bit 0 (digitalSignature) is
    // the high bit of the first content byte.
    let mut bits: u16 = 0;
    for (byte_idx, &b) in bs.value[1..].iter().enumerate().take(2) {
        for bit in 0..8 {
            // Skip the unused trailing bits in the LAST byte.
            if byte_idx == bs.value.len() - 2
                && bit >= 8u32.saturating_sub(unused_bits) as usize {
                break;
            }
            if b & (0x80 >> bit) != 0 {
                let pos = byte_idx * 8 + bit;
                if pos < 16 {
                    bits |= 1 << pos;
                }
            }
        }
    }
    Some(bits)
}

fn parse_eku(value: &[u8], server_auth: &mut bool, any: &mut bool) {
    let (seq, _) = match asn1::parse_tlv(value) {
        Some(t) => t,
        None => return,
    };
    if seq.tag != TAG_SEQUENCE { return; }
    for purpose in asn1::parse_sequence_contents(seq.value) {
        if purpose.tag != TAG_OID { continue; }
        if purpose.value == OID_KP_SERVER_AUTH { *server_auth = true; }
        else if purpose.value == OID_KP_ANY { *any = true; }
    }
}
