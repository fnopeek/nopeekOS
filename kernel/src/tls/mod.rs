//! TLS 1.3 (RFC 8446)
//!
//! Minimal implementation for HTTPS client connections.
//! Cipher suites: TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384,
//!                TLS_CHACHA20_POLY1305_SHA256

pub mod sha256;
pub mod hmac;
pub mod x25519;
pub mod rsa;
pub mod asn1;
pub mod x509;
pub mod certstore;

use alloc::vec::Vec;
use sha256::Sha256;
use crate::{crypto, csprng, net::tcp};

// TLS 1.3 constants
const TLS_VERSION_12: [u8; 2] = [0x03, 0x03]; // Record layer uses 1.2
const TLS_VERSION_13: [u8; 2] = [0x03, 0x04]; // Supported versions extension

// Content types
const CT_CHANGE_CIPHER_SPEC: u8 = 0x14;
const CT_ALERT: u8 = 0x15;
const CT_HANDSHAKE: u8 = 0x16;
const CT_APPLICATION_DATA: u8 = 0x17;

// Handshake types
const HT_CLIENT_HELLO: u8 = 0x01;
const HT_SERVER_HELLO: u8 = 0x02;
const HT_ENCRYPTED_EXTENSIONS: u8 = 0x08;
const HT_CERTIFICATE: u8 = 0x0B;
const HT_CERTIFICATE_VERIFY: u8 = 0x0F;
const HT_FINISHED: u8 = 0x14;

// Extension types
const EXT_SERVER_NAME: u16 = 0x0000;
const EXT_SUPPORTED_VERSIONS: u16 = 0x002B;
const EXT_KEY_SHARE: u16 = 0x0033;
const EXT_SUPPORTED_GROUPS: u16 = 0x000A;
const EXT_SIGNATURE_ALGORITHMS: u16 = 0x000D;

// Named groups
const GROUP_SECP384R1: u16 = 0x0018;
const GROUP_X25519: u16 = 0x001D;

enum ServerKeyShare {
    X25519([u8; 32]),
    Secp384r1(Vec<u8>), // 97 bytes uncompressed point
}

// Max TLS record payload
const MAX_RECORD_PAYLOAD: usize = 16384 + 256; // 16KB + overhead

// ============================================================
// Cipher Suite
// ============================================================

#[derive(Clone, Copy, PartialEq)]
pub enum CipherSuite {
    Aes128Gcm,         // TLS_AES_128_GCM_SHA256 (0x1301)
    Aes256Gcm,         // TLS_AES_256_GCM_SHA384 (0x1302)
    Chacha20Poly1305,  // TLS_CHACHA20_POLY1305_SHA256 (0x1303)
}

impl CipherSuite {
    fn key_len(self) -> usize {
        match self {
            CipherSuite::Aes128Gcm => 16,
            _ => 32,
        }
    }

    fn hash_len(self) -> usize {
        match self {
            CipherSuite::Aes256Gcm => 48,
            _ => 32,
        }
    }

    fn name(self) -> &'static str {
        match self {
            CipherSuite::Aes128Gcm => "AES-128-GCM",
            CipherSuite::Aes256Gcm => "AES-256-GCM",
            CipherSuite::Chacha20Poly1305 => "ChaCha20-Poly1305",
        }
    }
}

// ============================================================
// Transcript Hash (SHA-256 or SHA-384 depending on cipher suite)
// ============================================================

#[derive(Clone)]
enum TranscriptHash {
    S256(Sha256),
    S384(sha256::Sha384),
}

impl TranscriptHash {
    fn new(cs: CipherSuite) -> Self {
        match cs {
            CipherSuite::Aes256Gcm => TranscriptHash::S384(sha256::Sha384::new()),
            _ => TranscriptHash::S256(Sha256::new()),
        }
    }

    fn update(&mut self, data: &[u8]) {
        match self {
            TranscriptHash::S256(h) => h.update(data),
            TranscriptHash::S384(h) => h.update(data),
        }
    }

    fn finalize(self) -> Vec<u8> {
        match self {
            TranscriptHash::S256(h) => h.finalize().to_vec(),
            TranscriptHash::S384(h) => h.finalize().to_vec(),
        }
    }
}

// ============================================================
// AEAD dispatch (ChaCha20-Poly1305, AES-128-GCM, AES-256-GCM)
// ============================================================

fn tls_aead_encrypt(cs: CipherSuite, key: &[u8], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    match cs {
        CipherSuite::Chacha20Poly1305 => {
            let mut k = [0u8; 32];
            k.copy_from_slice(key);
            crypto::aead_encrypt_aad(&k, nonce, aad, plaintext)
        }
        CipherSuite::Aes128Gcm => {
            use aes_gcm::{Aes128Gcm, Nonce};
            use aes_gcm::aead::{Aead, KeyInit, Payload};
            let cipher = Aes128Gcm::new_from_slice(key).expect("AES-128 key");
            cipher.encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
                .expect("AES-128-GCM encrypt")
        }
        CipherSuite::Aes256Gcm => {
            use aes_gcm::{Aes256Gcm, Nonce};
            use aes_gcm::aead::{Aead, KeyInit, Payload};
            let cipher = Aes256Gcm::new_from_slice(key).expect("AES-256 key");
            cipher.encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
                .expect("AES-256-GCM encrypt")
        }
    }
}

fn tls_aead_decrypt(cs: CipherSuite, key: &[u8], nonce: &[u8; 12], aad: &[u8], ciphertext: &[u8]) -> Option<Vec<u8>> {
    match cs {
        CipherSuite::Chacha20Poly1305 => {
            let mut k = [0u8; 32];
            k.copy_from_slice(key);
            crypto::aead_decrypt_aad(&k, nonce, aad, ciphertext)
        }
        CipherSuite::Aes128Gcm => {
            use aes_gcm::{Aes128Gcm, Nonce};
            use aes_gcm::aead::{Aead, KeyInit, Payload};
            let cipher = Aes128Gcm::new_from_slice(key).ok()?;
            cipher.decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad }).ok()
        }
        CipherSuite::Aes256Gcm => {
            use aes_gcm::{Aes256Gcm, Nonce};
            use aes_gcm::aead::{Aead, KeyInit, Payload};
            let cipher = Aes256Gcm::new_from_slice(key).ok()?;
            cipher.decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad }).ok()
        }
    }
}

// ============================================================
// Key Schedule dispatch (SHA-256 or SHA-384)
// ============================================================

fn ks_empty_hash(cs: CipherSuite) -> Vec<u8> {
    match cs {
        CipherSuite::Aes256Gcm => sha256::sha384(&[]).to_vec(),
        _ => sha256::sha256(&[]).to_vec(),
    }
}

fn ks_extract(cs: CipherSuite, salt: &[u8], ikm: &[u8]) -> Vec<u8> {
    match cs {
        CipherSuite::Aes256Gcm => hmac::hkdf_extract_384(salt, ikm).to_vec(),
        _ => hmac::hkdf_extract(salt, ikm).to_vec(),
    }
}

fn ks_derive_secret(cs: CipherSuite, secret: &[u8], label: &[u8], hash: &[u8]) -> Vec<u8> {
    match cs {
        CipherSuite::Aes256Gcm => {
            let mut s = [0u8; 48];
            s.copy_from_slice(secret);
            let mut h = [0u8; 48];
            h.copy_from_slice(hash);
            hmac::derive_secret_384(&s, label, &h).to_vec()
        }
        _ => {
            let mut s = [0u8; 32];
            s.copy_from_slice(secret);
            let mut h = [0u8; 32];
            h.copy_from_slice(hash);
            hmac::derive_secret(&s, label, &h).to_vec()
        }
    }
}

fn ks_expand_key(cs: CipherSuite, secret: &[u8]) -> Vec<u8> {
    let len = cs.key_len();
    match cs {
        CipherSuite::Aes256Gcm => {
            let mut s = [0u8; 48];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label_384(&s, b"key", &[], len)
        }
        _ => {
            let mut s = [0u8; 32];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label(&s, b"key", &[], len)
        }
    }
}

fn ks_expand_iv(cs: CipherSuite, secret: &[u8]) -> [u8; 12] {
    let expanded = match cs {
        CipherSuite::Aes256Gcm => {
            let mut s = [0u8; 48];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label_384(&s, b"iv", &[], 12)
        }
        _ => {
            let mut s = [0u8; 32];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label(&s, b"iv", &[], 12)
        }
    };
    let mut iv = [0u8; 12];
    iv.copy_from_slice(&expanded);
    iv
}

fn ks_finished_key(cs: CipherSuite, secret: &[u8]) -> Vec<u8> {
    let len = cs.hash_len();
    match cs {
        CipherSuite::Aes256Gcm => {
            let mut s = [0u8; 48];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label_384(&s, b"finished", &[], len)
        }
        _ => {
            let mut s = [0u8; 32];
            s.copy_from_slice(secret);
            hmac::hkdf_expand_label(&s, b"finished", &[], len)
        }
    }
}

fn ks_hmac(cs: CipherSuite, key: &[u8], msg: &[u8]) -> Vec<u8> {
    match cs {
        CipherSuite::Aes256Gcm => hmac::hmac_sha384(key, msg).to_vec(),
        _ => hmac::hmac_sha256(key, msg).to_vec(),
    }
}

// ============================================================
// P-384 ECDH Key Exchange
// ============================================================

struct P384KeyPair {
    secret: p384::SecretKey,
    public_uncompressed: [u8; 97], // 0x04 || x(48) || y(48)
}

fn p384_keygen() -> P384KeyPair {
    // Generate 48 random bytes from CSPRNG
    let r1 = csprng::random_256();
    let r2 = csprng::random_256();
    let mut key_bytes = [0u8; 48];
    key_bytes[..32].copy_from_slice(&r1);
    key_bytes[32..].copy_from_slice(&r2[..16]);

    // Retry if invalid (zero or >= curve order)
    let secret = loop {
        if let Ok(sk) = p384::SecretKey::from_slice(&key_bytes) {
            break sk;
        }
        // Reseed and retry (extremely unlikely)
        let r = csprng::random_256();
        key_bytes[..32].copy_from_slice(&r);
    };

    // Compute public key (uncompressed point)
    use p384::elliptic_curve::sec1::ToEncodedPoint;
    let pub_point = secret.public_key().to_encoded_point(false);
    let pub_bytes = pub_point.as_bytes();
    let mut public_uncompressed = [0u8; 97];
    public_uncompressed.copy_from_slice(&pub_bytes[..97]);

    P384KeyPair { secret, public_uncompressed }
}

fn p384_ecdh(secret: &p384::SecretKey, server_pub_bytes: &[u8]) -> Result<Vec<u8>, TlsError> {
    use p384::elliptic_curve::sec1::FromEncodedPoint;

    let server_point = p384::EncodedPoint::from_bytes(server_pub_bytes)
        .map_err(|_| TlsError::HandshakeFailed("invalid P-384 point"))?;
    let server_pk: Option<p384::PublicKey> = p384::PublicKey::from_encoded_point(&server_point).into();
    let server_pk = server_pk
        .ok_or(TlsError::HandshakeFailed("P-384 point not on curve"))?;

    let shared = p384::ecdh::diffie_hellman(
        secret.to_nonzero_scalar(),
        server_pk.as_affine(),
    );
    Ok(shared.raw_secret_bytes().to_vec())
}

// ============================================================
// TLS Session
// ============================================================

pub struct TlsSession {
    tcp_handle: usize,
    cipher: CipherSuite,
    client_app_key: [u8; 32], // first cipher.key_len() bytes used
    server_app_key: [u8; 32],
    client_app_iv: [u8; 12],
    server_app_iv: [u8; 12],
    client_seq: u64,
    server_seq: u64,
}

impl TlsSession {
    pub fn cipher_name(&self) -> &'static str {
        self.cipher.name()
    }
}

#[derive(Debug)]
pub enum TlsError {
    Tcp(tcp::TcpError),
    HandshakeFailed(&'static str),
    CertificateError(certstore::CertError),
    DecryptError,
    UnexpectedMessage,
    RecordTooLarge,
}

impl core::fmt::Display for TlsError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            TlsError::Tcp(e) => write!(f, "TCP: {}", e),
            TlsError::HandshakeFailed(s) => write!(f, "handshake: {}", s),
            TlsError::CertificateError(e) => write!(f, "certificate: {}", e),
            TlsError::DecryptError => write!(f, "decryption failed"),
            TlsError::UnexpectedMessage => write!(f, "unexpected message"),
            TlsError::RecordTooLarge => write!(f, "record too large"),
        }
    }
}

impl From<tcp::TcpError> for TlsError {
    fn from(e: tcp::TcpError) -> Self { TlsError::Tcp(e) }
}

/// Establish a TLS 1.3 connection over an existing TCP handle.
pub fn tls_connect(tcp_handle: usize, hostname: &str) -> Result<TlsSession, TlsError> {
    // Generate ephemeral key pairs for both groups
    let x25519_private = csprng::random_256();
    let x25519_public = x25519::x25519_base(&x25519_private);
    let p384_keypair = p384_keygen();
    let client_random = csprng::random_256();

    // === ClientHello ===
    let client_hello = build_client_hello(&client_random, &x25519_public, &p384_keypair.public_uncompressed, hostname);
    send_record(tcp_handle, CT_HANDSHAKE, &client_hello)?;

    // === ServerHello ===
    let server_hello = recv_handshake_message(tcp_handle, HT_SERVER_HELLO)?;
    let (server_key_share, cipher) = parse_server_hello(&server_hello)?;

    // Transcript hash with correct algorithm (determined by cipher suite)
    let mut transcript = TranscriptHash::new(cipher);
    transcript.update(&client_hello);
    transcript.update(&server_hello);

    // === Derive handshake keys ===
    let shared_secret = match server_key_share {
        ServerKeyShare::X25519(server_pub) => {
            x25519::x25519(&x25519_private, &server_pub).to_vec()
        }
        ServerKeyShare::Secp384r1(server_pub) => {
            p384_ecdh(&p384_keypair.secret, &server_pub)?
        }
    };

    let empty_hash = ks_empty_hash(cipher);
    let zero_ikm = alloc::vec![0u8; cipher.hash_len()];
    let early_secret = ks_extract(cipher, &zero_ikm, &zero_ikm);
    let derived = ks_derive_secret(cipher, &early_secret, b"derived", &empty_hash);
    let handshake_secret = ks_extract(cipher, &derived, &shared_secret);

    // Transcript hash up to ServerHello
    let mut transcript_sh = TranscriptHash::new(cipher);
    transcript_sh.update(&client_hello);
    transcript_sh.update(&server_hello);
    let sh_hash = transcript_sh.finalize();

    // Client/Server Handshake Traffic Secrets
    let client_hs_secret = ks_derive_secret(cipher, &handshake_secret, b"c hs traffic", &sh_hash);
    let server_hs_secret = ks_derive_secret(cipher, &handshake_secret, b"s hs traffic", &sh_hash);

    // Handshake keys
    let server_hs_key = ks_expand_key(cipher, &server_hs_secret);
    let server_hs_iv = ks_expand_iv(cipher, &server_hs_secret);
    let client_hs_key = ks_expand_key(cipher, &client_hs_secret);
    let client_hs_iv = ks_expand_iv(cipher, &client_hs_secret);

    let mut server_hs_seq: u64 = 0;

    // === Receive encrypted handshake messages ===
    // May receive ChangeCipherSpec (legacy, ignore)
    // Then: EncryptedExtensions, Certificate, CertificateVerify, Finished

    let mut cert_chain: Vec<Vec<u8>> = Vec::new();
    let mut _cert_verify_sig: Vec<u8> = Vec::new();
    let mut _cert_verify_algo: u16 = 0;
    let mut server_finished: Vec<u8> = Vec::new();

    loop {
        let (ct, record) = recv_record(tcp_handle)?;

        if ct == CT_CHANGE_CIPHER_SPEC {
            continue;
        }
        if ct != CT_APPLICATION_DATA {
            return Err(TlsError::UnexpectedMessage);
        }

        let nonce = build_nonce(&server_hs_iv, server_hs_seq);
        server_hs_seq += 1;
        let aad = build_record_aad(CT_APPLICATION_DATA, record.len());
        let plaintext = tls_aead_decrypt(cipher, &server_hs_key, &nonce, &aad, &record)
            .ok_or(TlsError::DecryptError)?;

        // Last byte of plaintext is the real content type
        if plaintext.is_empty() {
            return Err(TlsError::DecryptError);
        }
        let real_ct = plaintext[plaintext.len() - 1];
        let inner = &plaintext[..plaintext.len() - 1];

        if real_ct != CT_HANDSHAKE {
            if real_ct == CT_ALERT {
                return Err(TlsError::HandshakeFailed("server alert"));
            }
            continue;
        }

        // Parse handshake messages from inner data
        let mut pos = 0;
        while pos + 4 <= inner.len() {
            let hs_type = inner[pos];
            let hs_len = ((inner[pos + 1] as usize) << 16)
                       | ((inner[pos + 2] as usize) << 8)
                       | (inner[pos + 3] as usize);
            let hs_end = pos + 4 + hs_len;
            if hs_end > inner.len() { break; }

            let hs_msg = &inner[pos..hs_end];

            match hs_type {
                HT_ENCRYPTED_EXTENSIONS => {
                    transcript.update(hs_msg);
                }
                HT_CERTIFICATE => {
                    transcript.update(hs_msg);
                    cert_chain = parse_certificate_message(&inner[pos + 4..hs_end]);
                }
                HT_CERTIFICATE_VERIFY => {
                    transcript.update(hs_msg);
                    if hs_len >= 4 {
                        _cert_verify_algo = ((inner[pos + 4] as u16) << 8) | inner[pos + 5] as u16;
                        let sig_len = ((inner[pos + 6] as usize) << 8) | inner[pos + 7] as usize;
                        if pos + 8 + sig_len <= hs_end {
                            _cert_verify_sig = inner[pos + 8..pos + 8 + sig_len].to_vec();
                        }
                    }
                }
                HT_FINISHED => {
                    // Do NOT add to transcript before verifying!
                    server_finished = inner[pos + 4..hs_end].to_vec();
                }
                _ => { /* Unknown, skip */ }
            }

            pos = hs_end;
        }

        if !server_finished.is_empty() {
            break;
        }
    }

    // === Verify certificate chain ===
    if cert_chain.is_empty() {
        return Err(TlsError::HandshakeFailed("no certificates"));
    }

    let cert_refs: Vec<&[u8]> = cert_chain.iter().map(|c| c.as_slice()).collect();
    certstore::verify_chain(&cert_refs, hostname)
        .map_err(TlsError::CertificateError)?;

    // === Verify Finished ===
    let transcript_before_sf = transcript.clone();
    let hs_hash = transcript.finalize();

    let finished_key = ks_finished_key(cipher, &server_hs_secret);
    let expected_finished = ks_hmac(cipher, &finished_key, &hs_hash);
    if server_finished.len() != cipher.hash_len() || !constant_time_eq(&server_finished, &expected_finished) {
        return Err(TlsError::HandshakeFailed("finished verify failed"));
    }

    // === Send Client Finished ===
    let hash_len = cipher.hash_len();

    // Client Finished verify_data uses transcript including server Finished
    let mut cf_transcript = transcript_before_sf.clone();
    let mut sf_hs_msg = Vec::new();
    sf_hs_msg.push(HT_FINISHED);
    sf_hs_msg.push(0); sf_hs_msg.push(0); sf_hs_msg.push(server_finished.len() as u8);
    sf_hs_msg.extend_from_slice(&server_finished);
    cf_transcript.update(&sf_hs_msg);
    let cf_hash = cf_transcript.finalize();

    let client_finished_key = ks_finished_key(cipher, &client_hs_secret);
    let client_finished_data = ks_hmac(cipher, &client_finished_key, &cf_hash);

    // Build the Finished handshake message
    let mut finished_msg = Vec::new();
    finished_msg.push(HT_FINISHED);
    finished_msg.push(0); finished_msg.push(0); finished_msg.push(hash_len as u8);
    finished_msg.extend_from_slice(&client_finished_data);

    // Encrypt and send Client Finished
    let client_nonce = build_nonce(&client_hs_iv, 0);
    let mut inner_with_ct = finished_msg.clone();
    inner_with_ct.push(CT_HANDSHAKE);
    let aad = build_record_aad(CT_APPLICATION_DATA, inner_with_ct.len() + 16);
    let encrypted = tls_aead_encrypt(cipher, &client_hs_key, &client_nonce, &aad, &inner_with_ct);
    send_record(tcp_handle, CT_APPLICATION_DATA, &encrypted)?;

    // === Derive Application Keys ===
    // App traffic secrets use Hash(CH..SF) — transcript including server Finished
    let mut app_transcript = transcript_before_sf;
    let mut sf_msg = Vec::new();
    sf_msg.push(HT_FINISHED);
    sf_msg.push(0); sf_msg.push(0); sf_msg.push(server_finished.len() as u8);
    sf_msg.extend_from_slice(&server_finished);
    app_transcript.update(&sf_msg);
    let app_hash = app_transcript.finalize();

    let derived2 = ks_derive_secret(cipher, &handshake_secret, b"derived", &empty_hash);
    let master_secret = ks_extract(cipher, &derived2, &zero_ikm);

    let client_app_secret = ks_derive_secret(cipher, &master_secret, b"c ap traffic", &app_hash);
    let server_app_secret = ks_derive_secret(cipher, &master_secret, b"s ap traffic", &app_hash);

    let client_key_vec = ks_expand_key(cipher, &client_app_secret);
    let server_key_vec = ks_expand_key(cipher, &server_app_secret);
    let client_app_iv = ks_expand_iv(cipher, &client_app_secret);
    let server_app_iv = ks_expand_iv(cipher, &server_app_secret);

    let mut client_app_key = [0u8; 32];
    client_app_key[..client_key_vec.len()].copy_from_slice(&client_key_vec);
    let mut server_app_key = [0u8; 32];
    server_app_key[..server_key_vec.len()].copy_from_slice(&server_key_vec);

    Ok(TlsSession {
        tcp_handle,
        cipher,
        client_app_key,
        server_app_key,
        client_app_iv,
        server_app_iv,
        client_seq: 0,
        server_seq: 0,
    })
}

/// Send application data over TLS.
pub fn tls_send(session: &mut TlsSession, data: &[u8]) -> Result<(), TlsError> {
    let mut inner = data.to_vec();
    inner.push(CT_APPLICATION_DATA); // Inner content type

    let nonce = build_nonce(&session.client_app_iv, session.client_seq);
    session.client_seq += 1;

    let key = &session.client_app_key[..session.cipher.key_len()];
    let aad = build_record_aad(CT_APPLICATION_DATA, inner.len() + 16);
    let encrypted = tls_aead_encrypt(session.cipher, key, &nonce, &aad, &inner);
    send_record(session.tcp_handle, CT_APPLICATION_DATA, &encrypted)?;
    Ok(())
}

/// Receive application data over TLS.
pub fn tls_recv(session: &mut TlsSession, buf: &mut [u8]) -> Result<usize, TlsError> {
    let (ct, record) = recv_record(session.tcp_handle)?;

    if ct == CT_CHANGE_CIPHER_SPEC {
        return Ok(0);
    }
    if ct != CT_APPLICATION_DATA {
        return Err(TlsError::UnexpectedMessage);
    }

    let nonce = build_nonce(&session.server_app_iv, session.server_seq);
    session.server_seq += 1;

    let key = &session.server_app_key[..session.cipher.key_len()];
    let aad = build_record_aad(CT_APPLICATION_DATA, record.len());
    let plaintext = tls_aead_decrypt(session.cipher, key, &nonce, &aad, &record)
        .ok_or(TlsError::DecryptError)?;

    if plaintext.is_empty() {
        return Ok(0);
    }

    let real_ct = plaintext[plaintext.len() - 1];
    let data = &plaintext[..plaintext.len() - 1];

    if real_ct == CT_ALERT {
        return Err(TlsError::HandshakeFailed("alert received"));
    }

    // Skip handshake messages (NewSessionTicket etc.) in app data phase
    if real_ct == CT_HANDSHAKE {
        return Ok(0);
    }

    let copy_len = data.len().min(buf.len());
    buf[..copy_len].copy_from_slice(&data[..copy_len]);
    Ok(copy_len)
}

/// Close TLS session.
pub fn tls_close(session: &mut TlsSession) -> Result<(), TlsError> {
    // Send close_notify alert
    let mut alert = Vec::new();
    alert.push(1); // warning
    alert.push(0); // close_notify
    alert.push(CT_ALERT); // inner content type

    let nonce = build_nonce(&session.client_app_iv, session.client_seq);
    session.client_seq += 1;

    let key = &session.client_app_key[..session.cipher.key_len()];
    let aad = build_record_aad(CT_APPLICATION_DATA, alert.len() + 16);
    let encrypted = tls_aead_encrypt(session.cipher, key, &nonce, &aad, &alert);
    let _ = send_record(session.tcp_handle, CT_APPLICATION_DATA, &encrypted);
    let _ = tcp::close(session.tcp_handle);
    Ok(())
}

// ============================================================
// Internal helpers
// ============================================================

fn build_client_hello(random: &[u8; 32], x25519_pub: &[u8; 32], p384_pub: &[u8; 97], hostname: &str) -> Vec<u8> {
    let mut extensions = Vec::new();

    // SNI extension
    let sni = build_sni_extension(hostname);
    extensions.extend_from_slice(&sni);

    // Supported Versions: TLS 1.3
    put_u16(&mut extensions, EXT_SUPPORTED_VERSIONS);
    put_u16(&mut extensions, 3); // length
    extensions.push(2); // list length
    extensions.push(TLS_VERSION_13[0]);
    extensions.push(TLS_VERSION_13[1]);

    // Supported Groups: secp384r1 + x25519
    put_u16(&mut extensions, EXT_SUPPORTED_GROUPS);
    put_u16(&mut extensions, 6); // 2 groups x 2 bytes + list_len(2)
    put_u16(&mut extensions, 4); // list length
    put_u16(&mut extensions, GROUP_SECP384R1);
    put_u16(&mut extensions, GROUP_X25519);

    // Key Share: both x25519 (36 bytes) and secp384r1 (101 bytes)
    // x25519 entry: group(2) + key_len(2) + key(32) = 36
    // P-384 entry: group(2) + key_len(2) + key(97) = 101
    // Total shares: 36 + 101 = 137
    let shares_len: u16 = 36 + 101;
    put_u16(&mut extensions, EXT_KEY_SHARE);
    put_u16(&mut extensions, shares_len + 2); // extension data: shares_len_field(2) + shares
    put_u16(&mut extensions, shares_len);     // client_shares length
    // secp384r1 key share (first = preferred)
    put_u16(&mut extensions, GROUP_SECP384R1);
    put_u16(&mut extensions, 97);
    extensions.extend_from_slice(p384_pub);
    // x25519 key share
    put_u16(&mut extensions, GROUP_X25519);
    put_u16(&mut extensions, 32);
    extensions.extend_from_slice(x25519_pub);

    // Signature Algorithms (offer both RSA and ECDSA for server compatibility)
    put_u16(&mut extensions, EXT_SIGNATURE_ALGORITHMS);
    put_u16(&mut extensions, 12); // extension data length
    put_u16(&mut extensions, 10); // list length
    put_u16(&mut extensions, 0x0403); // ecdsa_secp256r1_sha256
    put_u16(&mut extensions, 0x0804); // rsa_pss_rsae_sha256 (TLS 1.3)
    put_u16(&mut extensions, 0x0401); // rsa_pkcs1_sha256
    put_u16(&mut extensions, 0x0503); // ecdsa_secp384r1_sha384
    put_u16(&mut extensions, 0x0805); // rsa_pss_rsae_sha384

    // Build ClientHello body
    let mut body = Vec::new();
    body.push(TLS_VERSION_12[0]); // Legacy version
    body.push(TLS_VERSION_12[1]);
    body.extend_from_slice(random); // 32 bytes random

    // Session ID (32 bytes random for TLS 1.3 compatibility mode)
    let session_id = csprng::random_256();
    body.push(32);
    body.extend_from_slice(&session_id);

    // Cipher suites: all 3 TLS 1.3 suites (strongest first)
    put_u16(&mut body, 6); // 3 suites x 2 bytes
    put_u16(&mut body, 0x1302); // TLS_AES_256_GCM_SHA384
    put_u16(&mut body, 0x1301); // TLS_AES_128_GCM_SHA256
    put_u16(&mut body, 0x1303); // TLS_CHACHA20_POLY1305_SHA256

    // Compression methods
    body.push(1); // Length
    body.push(0); // null compression

    // Extensions
    put_u16(&mut body, extensions.len() as u16);
    body.extend_from_slice(&extensions);

    // Wrap in handshake header
    let mut msg = Vec::new();
    msg.push(HT_CLIENT_HELLO);
    put_u24(&mut msg, body.len());
    msg.extend_from_slice(&body);
    msg
}

fn build_sni_extension(hostname: &str) -> Vec<u8> {
    let name = hostname.as_bytes();
    let mut ext = Vec::new();
    put_u16(&mut ext, EXT_SERVER_NAME);
    put_u16(&mut ext, (name.len() + 5) as u16); // extension data length
    put_u16(&mut ext, (name.len() + 3) as u16); // server name list length
    ext.push(0); // host_name type
    put_u16(&mut ext, name.len() as u16);
    ext.extend_from_slice(name);
    ext
}

fn parse_server_hello(msg: &[u8]) -> Result<(ServerKeyShare, CipherSuite), TlsError> {
    if msg.len() < 4 { return Err(TlsError::HandshakeFailed("ServerHello too short")); }
    let mut pos = 4; // Skip handshake header

    if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no version")); }
    pos += 2; // version

    if pos + 32 > msg.len() { return Err(TlsError::HandshakeFailed("no random")); }
    pos += 32; // server random

    if pos >= msg.len() { return Err(TlsError::HandshakeFailed("no session id len")); }
    let sid_len = msg[pos] as usize;
    pos += 1 + sid_len;

    // Cipher suite selected by server
    if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no cipher suite")); }
    let cipher = match (msg[pos], msg[pos + 1]) {
        (0x13, 0x01) => CipherSuite::Aes128Gcm,
        (0x13, 0x02) => CipherSuite::Aes256Gcm,
        (0x13, 0x03) => CipherSuite::Chacha20Poly1305,
        _ => return Err(TlsError::HandshakeFailed("unsupported cipher suite")),
    };
    pos += 2;

    pos += 1; // compression

    // Extensions
    if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no extensions")); }
    let ext_len = ((msg[pos] as usize) << 8) | msg[pos + 1] as usize;
    pos += 2;

    let ext_end = pos + ext_len;
    let mut key_share: Option<ServerKeyShare> = None;

    while pos + 4 <= ext_end && pos + 4 <= msg.len() {
        let ext_type = ((msg[pos] as u16) << 8) | msg[pos + 1] as u16;
        let data_len = ((msg[pos + 2] as usize) << 8) | msg[pos + 3] as usize;
        pos += 4;

        if ext_type == EXT_KEY_SHARE && data_len >= 4 {
            // group(2) + key_len(2) + key(N)
            let group = ((msg[pos] as u16) << 8) | msg[pos + 1] as u16;
            let key_len = ((msg[pos + 2] as usize) << 8) | msg[pos + 3] as usize;
            let key_start = pos + 4;
            if key_start + key_len <= msg.len() {
                match group {
                    GROUP_X25519 if key_len == 32 => {
                        let mut k = [0u8; 32];
                        k.copy_from_slice(&msg[key_start..key_start + 32]);
                        key_share = Some(ServerKeyShare::X25519(k));
                    }
                    GROUP_SECP384R1 if key_len == 97 => {
                        key_share = Some(ServerKeyShare::Secp384r1(
                            msg[key_start..key_start + 97].to_vec()
                        ));
                    }
                    _ => {}
                }
            }
        }

        pos += data_len;
    }

    match key_share {
        Some(ks) => Ok((ks, cipher)),
        None => Err(TlsError::HandshakeFailed("no server key_share")),
    }
}

fn parse_certificate_message(data: &[u8]) -> Vec<Vec<u8>> {
    let mut certs = Vec::new();
    if data.len() < 4 { return certs; }

    let mut pos = 0;

    // Request context (1 byte length + context)
    if pos >= data.len() { return certs; }
    let ctx_len = data[pos] as usize;
    pos += 1 + ctx_len;

    // Certificate list (3-byte length)
    if pos + 3 > data.len() { return certs; }
    let list_len = ((data[pos] as usize) << 16) | ((data[pos + 1] as usize) << 8) | data[pos + 2] as usize;
    pos += 3;
    let list_end = (pos + list_len).min(data.len());

    while pos + 3 < list_end {
        let cert_len = ((data[pos] as usize) << 16) | ((data[pos + 1] as usize) << 8) | data[pos + 2] as usize;
        pos += 3;
        if pos + cert_len > list_end { break; }
        certs.push(data[pos..pos + cert_len].to_vec());
        pos += cert_len;

        // Skip extensions (2-byte length + data)
        if pos + 2 <= list_end {
            let ext_len = ((data[pos] as usize) << 8) | data[pos + 1] as usize;
            pos += 2 + ext_len;
        }
    }

    certs
}

fn send_record(handle: usize, content_type: u8, payload: &[u8]) -> Result<(), TlsError> {
    let mut record = Vec::with_capacity(5 + payload.len());
    record.push(content_type);
    record.push(TLS_VERSION_12[0]);
    record.push(TLS_VERSION_12[1]);
    put_u16(&mut record, payload.len() as u16);
    record.extend_from_slice(payload);
    tcp::send(handle, &record)?;
    Ok(())
}

fn recv_record(handle: usize) -> Result<(u8, Vec<u8>), TlsError> {
    // Read 5-byte header
    let mut header = [0u8; 5];
    recv_exact(handle, &mut header)?;

    let content_type = header[0];
    let length = ((header[3] as usize) << 8) | header[4] as usize;

    if length > MAX_RECORD_PAYLOAD {
        return Err(TlsError::RecordTooLarge);
    }

    let mut payload = alloc::vec![0u8; length];
    recv_exact(handle, &mut payload)?;

    Ok((content_type, payload))
}

fn recv_exact(handle: usize, buf: &mut [u8]) -> Result<(), TlsError> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = tcp::recv_blocking(handle, &mut buf[filled..], 1000)?; // 10s timeout
        if n == 0 {
            return Err(TlsError::HandshakeFailed("connection closed"));
        }
        filled += n;
    }
    Ok(())
}

fn recv_handshake_message(handle: usize, expected_type: u8) -> Result<Vec<u8>, TlsError> {
    let (ct, payload) = recv_record(handle)?;
    if ct != CT_HANDSHAKE {
        if ct == CT_ALERT && payload.len() >= 2 {
            return Err(TlsError::HandshakeFailed(match payload[1] {
                40 => "server rejected handshake (alert 40)",
                70 => "protocol version not supported",
                71 => "insufficient security",
                _ => "server sent alert",
            }));
        }
        return Err(TlsError::UnexpectedMessage);
    }
    if payload.is_empty() || payload[0] != expected_type {
        return Err(TlsError::UnexpectedMessage);
    }
    Ok(payload)
}

fn build_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *iv;
    let seq_bytes = seq.to_be_bytes();
    // XOR sequence number into the last 8 bytes of IV
    for i in 0..8 {
        nonce[4 + i] ^= seq_bytes[i];
    }
    nonce
}

fn build_record_aad(content_type: u8, length: usize) -> [u8; 5] {
    [
        content_type,
        TLS_VERSION_12[0],
        TLS_VERSION_12[1],
        (length >> 8) as u8,
        length as u8,
    ]
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn put_u16(buf: &mut Vec<u8>, val: u16) {
    buf.push((val >> 8) as u8);
    buf.push(val as u8);
}

fn put_u24(buf: &mut Vec<u8>, val: usize) {
    buf.push((val >> 16) as u8);
    buf.push((val >> 8) as u8);
    buf.push(val as u8);
}
