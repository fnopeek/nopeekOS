//! Certificate Store
//!
//! Trusted root CA anchors + chain validation.
//!
//! Two tiers, deliberately: a built-in floor compiled into the signed
//! kernel, and a store of DER files under `sys/certs/` that arrives as
//! signed OTA assets or is added by hand. The floor exists so a broken,
//! empty or hostile store can never cut the machine off from its own
//! updates — the anchors needed to reach the update host are code, and
//! code cannot go missing. Everything above that is data.

use alloc::vec::Vec;
use alloc::string::String;
use spin::Mutex;

use super::x509::{self, X509Cert, KeyType, KU_DIGITAL_SIGNATURE, KU_KEY_CERT_SIGN};
use super::sha256;

/// ISRG Root X1 (Let's Encrypt) — covers ~60% of the web
const ISRG_ROOT_X1_DER: &[u8] = include_bytes!("../../../certs/isrg_root_x1.der");

/// DigiCert Global Root G2 — covers Anthropic, Cloudflare, etc.
const DIGICERT_GLOBAL_G2_DER: &[u8] = include_bytes!("../../../certs/digicert_global_g2.der");

/// AAA Certificate Services (Comodo/Sectigo) — covers Cloudflare default certs
const AAA_CERT_SERVICES_DER: &[u8] = include_bytes!("../../../certs/aaa_certificate_services.der");

/// Google Trust Services Root R1 — covers Google services
const GTS_ROOT_R1_DER: &[u8] = include_bytes!("../../../certs/gts_root_r1.der");

/// USERTrust ECC Certification Authority — Sectigo's modern ECC root.
/// Sectigo cross-signs newer roots (Public Server Authentication Root
/// E46) under USERTrust ECC, so adding the cross-anchor here covers
/// github.com + most Sectigo-issued ECDSA certs in 2025+.
const USERTRUST_ECC_DER: &[u8] = include_bytes!("../../../certs/usertrust_ecc.der");

/// USERTrust RSA Certification Authority — Sectigo's modern RSA root.
/// Counterpart to USERTrust ECC for RSA chains. Covers Sectigo
/// Public Server Authentication Root R46 + a wide RSA customer base.
const USERTRUST_RSA_DER: &[u8] = include_bytes!("../../../certs/usertrust_rsa.der");

/// Amazon Root CA 1 — anchors CloudFront, which fronts a large share of
/// the web (doc.rust-lang.org among them). Its absence was measured, not
/// guessed: those sites failed with `certificate: untrusted root CA`.
const AMAZON_ROOT_CA1_DER: &[u8] = include_bytes!("../../../certs/amazon_root_ca1.der");

/// ISRG Root X2 — Let's Encrypt's ECDSA hierarchy, a separate anchor from
/// X1. Servers that chain to X2 rather than offering an X1-anchored
/// variant were unreachable with X1 alone.
const ISRG_ROOT_X2_DER: &[u8] = include_bytes!("../../../certs/isrg_root_x2.der");

/// Built-in anchors. This set is the FLOOR: it ships inside the signed
/// kernel, cannot be removed by an update or by the user, and is what
/// guarantees the update host stays reachable even when the npkFS store
/// is empty, stale, or broken. Everything else is delivered as data —
/// see [`store_roots`].
const ROOT_CERTS: &[&[u8]] = &[
    ISRG_ROOT_X1_DER,
    ISRG_ROOT_X2_DER,
    DIGICERT_GLOBAL_G2_DER,
    AAA_CERT_SERVICES_DER,
    GTS_ROOT_R1_DER,
    USERTRUST_ECC_DER,
    USERTRUST_RSA_DER,
    AMAZON_ROOT_CA1_DER,
];

/// npkFS directory holding the data-delivered anchors. Off limits to WASM
/// apps — write access here is the power to mint a MITM anchor for the
/// whole system, so the guard that protects the module store covers this
/// path too (`wasm.rs::is_trust_critical_path`).
pub const STORE_DIR: &str = "sys/certs";

/// A cap on the store, so a corrupt or hostile directory cannot exhaust
/// kernel memory during the boot load.
const MAX_STORE_ROOTS: usize = 64;
const MAX_ROOT_BYTES: usize = 8 * 1024;

/// Anchors loaded from [`STORE_DIR`].
///
/// Held in memory and refreshed explicitly, never read from npkFS during a
/// handshake: `verify_chain` runs mid-TLS, and a handshake can be raised
/// while an npkFS write is in flight, so touching the filesystem from here
/// would buy a lock-order problem for nothing.
static STORE_ROOTS: Mutex<Vec<StoreRoot>> = Mutex::new(Vec::new());

pub struct StoreRoot {
    pub name: String,
    pub der: Vec<u8>,
}

/// (Re)load `sys/certs/` into the in-memory anchor set. Call after
/// `npkfs::mount`, and after anything changes that directory (`cert
/// add`/`remove`, an asset update). Returns how many anchors are live.
///
/// A file that does not parse as X.509 is skipped with a log line rather
/// than failing the load — one bad file must not take the rest of the
/// store down with it.
pub fn load_store() -> usize {
    let entries = match crate::npkfs::fs::list(STORE_DIR) {
        Ok(Some(e)) => e,
        // No directory yet is the normal state on a fresh install, not an
        // error: the built-in floor carries the machine until assets land.
        Ok(None) => { STORE_ROOTS.lock().clear(); return 0; }
        Err(_) => {
            crate::kprintln!("[npk] certstore: {} unreadable, built-in anchors only", STORE_DIR);
            STORE_ROOTS.lock().clear();
            return 0;
        }
    };

    let mut loaded: Vec<StoreRoot> = Vec::new();
    for entry in entries.iter() {
        if loaded.len() >= MAX_STORE_ROOTS {
            crate::kprintln!("[npk] certstore: more than {} anchors in {}, rest ignored",
                MAX_STORE_ROOTS, STORE_DIR);
            break;
        }
        if entry.size as usize > MAX_ROOT_BYTES {
            crate::kprintln!("[npk] certstore: '{}' too large ({} B), skipped",
                entry.name, entry.size);
            continue;
        }
        let path = alloc::format!("{}/{}", STORE_DIR, entry.name);
        let der = match crate::npkfs::fs::read(&path) {
            Ok(Some(d)) => d,
            _ => continue,
        };
        // Parse before trusting: an anchor that cannot be read is an anchor
        // that would silently never match, which looks like a network fault
        // three layers up.
        if x509::parse_x509(&der).is_none() {
            crate::kprintln!("[npk] certstore: '{}' is not a valid certificate, skipped", entry.name);
            continue;
        }
        loaded.push(StoreRoot { name: entry.name.clone(), der });
    }

    let n = loaded.len();
    *STORE_ROOTS.lock() = loaded;
    if n > 0 {
        crate::kprintln!("[npk] certstore: {} built-in + {} stored anchor(s)", ROOT_CERTS.len(), n);
    }
    n
}

/// Number of built-in anchors — the floor that cannot be removed.
pub fn builtin_count() -> usize { ROOT_CERTS.len() }

/// What an anchor actually is, for showing a human before they trust it.
pub struct CertInfo {
    pub subject: String,
    pub issuer: String,
    /// UTC seconds, or None when the date could not be decoded.
    pub not_before: Option<u64>,
    pub not_after: Option<u64>,
    pub is_ca: bool,
    /// Subject == issuer AND the signature verifies against its own key.
    pub self_signed: bool,
    pub fingerprint: [u8; 32],
}

/// Describe a DER certificate. `None` if it does not parse as X.509.
pub fn describe(der: &[u8]) -> Option<CertInfo> {
    let c = x509::parse_x509(der)?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    Some(CertInfo {
        subject: text(c.subject_cn),
        issuer: text(c.issuer_cn),
        not_before: parse_asn1_time(c.not_before),
        not_after: parse_asn1_time(c.not_after),
        is_ca: c.is_ca,
        self_signed: c.subject_cn == c.issuer_cn && verify_signature(&c, &c),
        fingerprint: sha256::sha256(der),
    })
}

/// Lowercase hex SHA-256, colon-separated — the form CAs publish, so it
/// can be compared against the vendor's page character by character.
pub fn fingerprint_hex(fp: &[u8; 32]) -> String {
    let mut s = String::with_capacity(32 * 3 - 1);
    for (i, b) in fp.iter().enumerate() {
        if i > 0 { s.push(':'); }
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0F) as usize] as char);
    }
    s
}

/// Run `f` over every anchor: built-ins first (as `None`), then the stored
/// ones with their filename. Used by `cert list` to show provenance.
pub fn for_each_anchor(mut f: impl FnMut(Option<&str>, &[u8])) {
    for der in ROOT_CERTS {
        f(None, der);
    }
    for root in STORE_ROOTS.lock().iter() {
        f(Some(&root.name), &root.der);
    }
}

/// Verify a certificate chain.
/// `chain` is ordered leaf-first: [leaf, intermediate, ...].
/// Returns Ok(()) if the chain validates to a trusted root.
pub fn verify_chain(chain: &[&[u8]], hostname: &str) -> Result<(), CertError> {
    if chain.is_empty() {
        return Err(CertError::EmptyChain);
    }

    // Parse leaf certificate
    let leaf = x509::parse_x509(chain[0]).ok_or(CertError::ParseError)?;

    // Wall clock, or None when nothing believable is available. Resolved
    // ONCE for the whole chain so a tick between two certs cannot make the
    // verdict depend on where in the chain the check happened to land.
    let now = now_unix();
    if let Some(now) = now {
        check_validity(&leaf, now)?;
    }

    // Hostname → CN/SAN match
    if !cn_matches(&leaf, hostname) {
        return Err(CertError::HostnameMismatch);
    }
    // Critical extension we don't understand → RFC 5280 §4.2 reject.
    if leaf.unknown_critical_ext {
        return Err(CertError::UnknownCriticalExt);
    }
    // KeyUsage: if present, must include digitalSignature (TLS 1.3 ECDHE_*).
    if let Some(ku) = leaf.key_usage {
        if ku & KU_DIGITAL_SIGNATURE == 0 {
            return Err(CertError::KeyUsageInvalid);
        }
    }
    // EKU: if present, must include serverAuth or anyExtendedKeyUsage.
    if leaf.eku_present && !leaf.eku_server_auth && !leaf.eku_any {
        return Err(CertError::EkuInvalid);
    }

    // Build chain: verify each cert is signed by the next.
    // For each issuer (CA), enforce CA-bit, KU keyCertSign, pathLen, and the
    // critical-extension rule. `inter_below` counts non-self CAs that the
    // current issuer sits above in the chain (excluding the leaf).
    let mut current = leaf;
    for i in 1..chain.len() {
        let issuer = x509::parse_x509(chain[i]).ok_or(CertError::ParseError)?;

        if !verify_signature(&current, &issuer) {
            return Err(CertError::SignatureInvalid);
        }

        // Issuer must assert CA via BasicConstraints.
        if !issuer.is_ca {
            return Err(CertError::NotCA);
        }
        // KeyUsage on a CA, if present, must include keyCertSign.
        if let Some(ku) = issuer.key_usage {
            if ku & KU_KEY_CERT_SIGN == 0 {
                return Err(CertError::KeyUsageInvalid);
            }
        }
        // pathLenConstraint applies to non-self-issued certs below this CA in
        // the chain (RFC 5280 §4.2.1.9). `i - 1` is the count of intermediate
        // CAs sitting between this issuer and the leaf.
        if let Some(plc) = issuer.path_len_constraint {
            let inter_below = (i as u32).saturating_sub(1);
            if inter_below > plc {
                return Err(CertError::PathLenExceeded);
            }
        }
        if issuer.unknown_critical_ext {
            return Err(CertError::UnknownCriticalExt);
        }
        // Intermediates are checked like the leaf. The trust ANCHOR is not:
        // a root is trusted by its key, and browsers deliberately do not
        // fail a chain over an anchor's own dates — otherwise a root aging
        // out would break every site under it even after the replacement
        // has been cross-signed.
        if let Some(now) = now {
            check_validity(&issuer, now)?;
        }

        current = issuer;
    }

    // The top of the chain must resolve to a trusted anchor. Built-in floor
    // first, then the data-delivered store — one shared test, so a stored
    // anchor is never held to a weaker standard than a compiled-in one.
    for root_der in ROOT_CERTS {
        if anchors_chain(&current, root_der) {
            return Ok(());
        }
    }
    for root in STORE_ROOTS.lock().iter() {
        if anchors_chain(&current, &root.der) {
            return Ok(());
        }
    }

    Err(CertError::UntrustedRoot)
}

// ── Validity dates ────────────────────────────────────────────────────
//
// Below this, the clock is not believable and the check is SKIPPED rather
// than enforced. A dead CMOS battery reads the year 2000; enforcing
// against that would reject every certificate on earth and take HTTPS
// down completely — a far worse failure than honouring a stale one. The
// floor only has to be late enough that a plausible clock is a useful
// clock: 2025-01-01.
const CLOCK_SANE_FLOOR: u64 = 1_735_689_600;

/// Current UTC seconds, or `None` when no source is trustworthy.
/// NTP first — it is the accurate one; CMOS is the offline fallback.
fn now_unix() -> Option<u64> {
    let t = crate::net::ntp::unix_time()
        .or_else(crate::drivers::rtc::read_unix_time)?;
    (t >= CLOCK_SANE_FLOOR).then_some(t)
}

/// Decode a DER ASN.1 time into UTC seconds.
///
/// The two encodings are told apart by length, which is sound because DER
/// pins the form (RFC 5280 §4.1.2.5 — seconds mandatory, always `Z`):
/// UTCTime is `YYMMDDHHMMSSZ` (13), GeneralizedTime `YYYYMMDDHHMMSSZ` (15).
fn parse_asn1_time(v: &[u8]) -> Option<u64> {
    let num = |b: &[u8]| -> Option<i64> {
        let mut n: i64 = 0;
        for c in b {
            if !c.is_ascii_digit() { return None; }
            n = n * 10 + (c - b'0') as i64;
        }
        Some(n)
    };

    let (year, rest) = match v.len() {
        13 => {
            // RFC 5280 §4.1.2.5.1: YY >= 50 means 19YY, below means 20YY.
            let yy = num(&v[0..2])?;
            (if yy >= 50 { 1900 + yy } else { 2000 + yy }, &v[2..])
        }
        15 => (num(&v[0..4])?, &v[4..]),
        _ => return None,
    };
    if rest[rest.len() - 1] != b'Z' { return None; }

    let month = num(&rest[0..2])?;
    let day = num(&rest[2..4])?;
    let hour = num(&rest[4..6])?;
    let min = num(&rest[6..8])?;
    let sec = num(&rest[8..10])?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day)
        || hour > 23 || min > 59 || sec > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    (secs >= 0).then(|| secs as u64)
}

/// Days since the Unix epoch for a civil date (Howard Hinnant's
/// `days_from_civil`, valid across the whole proleptic Gregorian range).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Check one certificate against `now`. A date we cannot decode is a
/// reason to reject: an unreadable validity period is not a valid one.
fn check_validity(cert: &X509Cert, now: u64) -> Result<(), CertError> {
    let nb = parse_asn1_time(cert.not_before).ok_or(CertError::BadValidityDate)?;
    let na = parse_asn1_time(cert.not_after).ok_or(CertError::BadValidityDate)?;
    if now < nb {
        return Err(CertError::NotYetValid);
    }
    if now > na {
        return Err(CertError::Expired);
    }
    Ok(())
}

/// Does `root_der` anchor a chain whose topmost cert is `current`?
fn anchors_chain(current: &X509Cert, root_der: &[u8]) -> bool {
    let root = match x509::parse_x509(root_der) {
        Some(r) => r,
        None => return false,
    };

    // Check if current cert's issuer matches root's subject
    if current.issuer_cn == root.subject_cn && verify_signature(current, &root) {
        return true;
    }

    // The last cert IS one of our trusted roots. Match it by IDENTITY —
    // same subject + same public key — NOT by verifying its own signature.
    // This is required for cross-signed roots: e.g. google.* now serves GTS
    // Root R1 cross-signed by GlobalSign Root CA (issuer != subject), so its
    // self-signature check fails against GTS R1's own key even though the key
    // IS our anchor. The chain up to `current` was already signature-verified
    // by the caller, and an anchor is trusted by its key (RFC 5280 §6.1 trust
    // anchor), so matching the embedded key is sufficient and correct. The
    // `verify_signature` arm keeps the classic self-signed path.
    if current.subject_cn == root.subject_cn {
        let key_is_anchor = current.key_type == root.key_type
            && current.public_key == root.public_key
            && current.rsa_exponent == root.rsa_exponent;
        if key_is_anchor || verify_signature(current, &root) {
            return true;
        }
    }

    false
}

// Signature algorithm OIDs — SHA-256 and SHA-384 only.
// SHA-1 (`1.2.840.113549.1.1.5`) is rejected: collision-broken since 2017,
// last accepted by mainstream CAs ~2016. We never verify root self-signatures
// (roots are matched by subject DN against the embedded set), so SHA-1 only
// matters for intermediate/leaf chain hops — and there it's a hard reject.
// 1.2.840.10045.4.3.2 = ecdsa-with-SHA256
const OID_ECDSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
// 1.2.840.10045.4.3.3 = ecdsa-with-SHA384
const OID_ECDSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];
// 1.2.840.113549.1.1.11 = sha256WithRSAEncryption
const OID_RSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];
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

/// Verify an ECDSA P-384 signature over raw data.
/// Computes SHA-384 ourselves, then uses PrehashVerifier (proven path on bare metal).
/// pubkey: 97-byte uncompressed SEC1 point.
/// data: the raw data that was signed.
/// signature: DER-encoded ECDSA signature.
pub fn verify_p384_sha384(pubkey: &[u8], data: &[u8], signature: &[u8]) -> bool {
    let digest = super::sha256::sha384(data);
    verify_p384_prehash_384(pubkey, &digest, signature)
}

/// Verify an ECDSA P-384 signature over a pre-computed SHA-384 digest.
/// Same path as TLS cert verification — proven on bare metal.
pub fn verify_p384_prehash_384(pubkey: &[u8], prehash: &[u8; 48], signature: &[u8]) -> bool {
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
    KeyUsageInvalid,
    EkuInvalid,
    PathLenExceeded,
    UnknownCriticalExt,
    Expired,
    NotYetValid,
    BadValidityDate,
}

impl CertError {
    /// A stable, static reason string. Static because the whole HTTP layer
    /// carries `&'static str` errors — that is what lets the real cause
    /// travel from here up to the browser instead of being flattened into
    /// "TLS handshake failed" at the first boundary.
    pub fn reason(&self) -> &'static str {
        match self {
            CertError::EmptyChain => "certificate: empty chain",
            CertError::ParseError => "certificate: parse error",
            CertError::HostnameMismatch => "certificate: hostname mismatch",
            CertError::SignatureInvalid => "certificate: invalid signature",
            CertError::NotCA => "certificate: intermediate is not a CA",
            CertError::UntrustedRoot => "certificate: untrusted root CA",
            CertError::KeyUsageInvalid => "certificate: keyUsage missing required bit",
            CertError::EkuInvalid => "certificate: EKU missing serverAuth",
            CertError::PathLenExceeded => "certificate: pathLenConstraint exceeded",
            CertError::UnknownCriticalExt => "certificate: unknown critical extension",
            CertError::Expired => "certificate: expired",
            CertError::NotYetValid => "certificate: not yet valid",
            CertError::BadValidityDate => "certificate: unreadable validity period",
        }
    }
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
            CertError::KeyUsageInvalid => write!(f, "keyUsage missing required bit"),
            CertError::EkuInvalid => write!(f, "EKU missing serverAuth"),
            CertError::PathLenExceeded => write!(f, "pathLenConstraint exceeded"),
            CertError::UnknownCriticalExt => write!(f, "unknown critical extension"),
            CertError::Expired => write!(f, "certificate expired"),
            CertError::NotYetValid => write!(f, "certificate not yet valid"),
            CertError::BadValidityDate => write!(f, "unreadable validity period"),
        }
    }
}
