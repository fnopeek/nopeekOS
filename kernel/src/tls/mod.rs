//! TLS 1.3 (RFC 8446)
//!
//! Minimal implementation for HTTPS client connections.
//! Single cipher suite: TLS_CHACHA20_POLY1305_SHA256 (0x1303)

pub mod sha256;
pub mod hmac;
pub mod x25519;
pub mod rsa;
pub mod asn1;
pub mod x509;
pub mod certstore;

use alloc::vec::Vec;
use sha256::Sha256;
use crate::{kprintln, crypto, csprng, net::tcp};

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

// Cipher suite: TLS_CHACHA20_POLY1305_SHA256
const CIPHER_SUITE: [u8; 2] = [0x13, 0x03];

// Extension types
const EXT_SERVER_NAME: u16 = 0x0000;
const EXT_SUPPORTED_VERSIONS: u16 = 0x002B;
const EXT_KEY_SHARE: u16 = 0x0033;
const EXT_SUPPORTED_GROUPS: u16 = 0x000A;
const EXT_SIGNATURE_ALGORITHMS: u16 = 0x000D;

// Named group: x25519
const GROUP_X25519: u16 = 0x001D;

// Max TLS record payload
const MAX_RECORD_PAYLOAD: usize = 16384 + 256; // 16KB + overhead

pub struct TlsSession {
    tcp_handle: usize,
    client_app_key: [u8; 32],
    server_app_key: [u8; 32],
    client_app_iv: [u8; 12],
    server_app_iv: [u8; 12],
    client_seq: u64,
    server_seq: u64,
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
    // Generate ephemeral X25519 key pair
    let private_key = csprng::random_256();
    let public_key = x25519::x25519_base(&private_key);
    let client_random = csprng::random_256();

    // Transcript hash (running SHA-256 over all handshake messages)
    let mut transcript = Sha256::new();

    // === ClientHello ===
    let client_hello = build_client_hello(&client_random, &public_key, hostname);
    transcript.update(&client_hello);
    send_record(tcp_handle, CT_HANDSHAKE, &client_hello)?;

    // === ServerHello ===
    let server_hello = recv_handshake_message(tcp_handle, HT_SERVER_HELLO)?;
    transcript.update(&server_hello);

    let server_public_key = parse_server_hello(&server_hello)?;
    kprintln!("[tls] server_pubkey[0..4]={:02x}{:02x}{:02x}{:02x} our_pubkey[0..4]={:02x}{:02x}{:02x}{:02x}",
        server_public_key[0], server_public_key[1], server_public_key[2], server_public_key[3],
        public_key[0], public_key[1], public_key[2], public_key[3]);

    // === Derive handshake keys ===
    let shared_secret = x25519::x25519(&private_key, &server_public_key);

    // === DEBUG: Test X25519 with RFC 7748 test vector ===
    {
        let alice_sk: [u8; 32] = [
            0x77,0x07,0x6d,0x0a,0x73,0x18,0xa5,0x7d,0x3c,0x16,0xc1,0x72,0x51,0xb2,0x66,0x45,
            0xdf,0x4c,0x2f,0x87,0xeb,0xc0,0x99,0x2a,0xb1,0x77,0xfb,0xa5,0x1d,0xb9,0x2c,0x2a,
        ];
        let bob_pk: [u8; 32] = [
            0xde,0x9e,0xdb,0x7d,0x7b,0x7d,0xc1,0xb4,0xd3,0x5b,0x61,0xc2,0xec,0xe4,0x35,0x37,
            0x3f,0x83,0x43,0xc8,0x5b,0x78,0x67,0x4d,0xad,0xfc,0x7e,0x14,0x6f,0x88,0x2b,0x4f,
        ];
        // Test 1: scalar = 1 (with clamping: bit 254 set, low bits cleared)
        // x25519([64,0,...,0], [9,0,...,0]) should NOT equal [9,0,...,0] (clamping changes scalar)
        // But x25519_base with the RFC vector should give the expected pubkey
        let shared = x25519::x25519(&alice_sk, &bob_pk);
        kprintln!("[tls] X25519 shared[0..8]={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} (expect 4a5d9d5b a4ce2de1)",
            shared[0], shared[1], shared[2], shared[3],
            shared[4], shared[5], shared[6], shared[7]);

        let alice_pk = x25519::x25519_base(&alice_sk);
        kprintln!("[tls] X25519 pubkey[0..8]={:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}{:02x} (expect 8520f009 8930a754)",
            alice_pk[0], alice_pk[1], alice_pk[2], alice_pk[3],
            alice_pk[4], alice_pk[5], alice_pk[6], alice_pk[7]);

        // Test 2: Simple scalar mul - scalar=all zeros with bit 254 set = [0,0,...,0,64]
        // After clamping this is just 2^254. x25519(2^254, 9) has a known result.
        let mut simple_sk = [0u8; 32];
        simple_sk[31] = 64; // bit 254
        let simple_pk = x25519::x25519_base(&simple_sk);
        kprintln!("[tls] X25519 simple(2^254, 9)[0..4]={:02x}{:02x}{:02x}{:02x}",
            simple_pk[0], simple_pk[1], simple_pk[2], simple_pk[3]);
    }

    // === DEBUG: Test AEAD with RFC 8439 test vector ===
    {
        let test_key: [u8; 32] = [
            0x80,0x81,0x82,0x83,0x84,0x85,0x86,0x87,0x88,0x89,0x8a,0x8b,0x8c,0x8d,0x8e,0x8f,
            0x90,0x91,0x92,0x93,0x94,0x95,0x96,0x97,0x98,0x99,0x9a,0x9b,0x9c,0x9d,0x9e,0x9f,
        ];
        let test_nonce: [u8; 12] = [0x07,0x00,0x00,0x00,0x40,0x41,0x42,0x43,0x44,0x45,0x46,0x47];
        let test_aad: &[u8] = &[0x50,0x51,0x52,0x53,0xc0,0xc1,0xc2,0xc3,0xc4,0xc5,0xc6,0xc7];
        let test_pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";

        let ct = crypto::aead_encrypt_aad(&test_key, &test_nonce, test_aad, test_pt);
        kprintln!("[tls] AEAD test: ct[0..4]={:02x}{:02x}{:02x}{:02x} tag[-4..]={:02x}{:02x}{:02x}{:02x} len={}",
            ct[0], ct[1], ct[2], ct[3],
            ct[ct.len()-4], ct[ct.len()-3], ct[ct.len()-2], ct[ct.len()-1],
            ct.len());
        // Expected: ct[0..4]=d31a8d34, tag[-4..]=d0600691, len=130

        let dec = crypto::aead_decrypt_aad(&test_key, &test_nonce, test_aad, &ct);
        kprintln!("[tls] AEAD decrypt: {}", if dec.is_some() { "OK" } else { "FAIL" });
    }
    // === END DEBUG ===

    let empty_hash = sha256::sha256(&[]);

    // Debug: verify SHA-256("") matches known value
    // Expected: e3b0c442 98fc1c14 9afbf4c8 996fb924
    kprintln!("[tls] SHA256('')={:02x}{:02x}{:02x}{:02x}...",
        empty_hash[0], empty_hash[1], empty_hash[2], empty_hash[3]);

    // Early Secret
    let early_secret = hmac::hkdf_extract(&[0u8; 32], &[0u8; 32]);
    kprintln!("[tls] early_secret[0..4]={:02x}{:02x}{:02x}{:02x}",
        early_secret[0], early_secret[1], early_secret[2], early_secret[3]);

    // Derived Secret
    let derived = hmac::derive_secret(&early_secret, b"derived", &empty_hash);
    kprintln!("[tls] derived[0..4]={:02x}{:02x}{:02x}{:02x}",
        derived[0], derived[1], derived[2], derived[3]);

    // Handshake Secret
    let handshake_secret = hmac::hkdf_extract(&derived, &shared_secret);
    kprintln!("[tls] hs_secret[0..4]={:02x}{:02x}{:02x}{:02x} shared[0..4]={:02x}{:02x}{:02x}{:02x}",
        handshake_secret[0], handshake_secret[1], handshake_secret[2], handshake_secret[3],
        shared_secret[0], shared_secret[1], shared_secret[2], shared_secret[3]);

    // Transcript hash up to ServerHello
    let mut transcript_clone = Sha256::new();
    transcript_clone.update(&client_hello);
    transcript_clone.update(&server_hello);
    let sh_hash = transcript_clone.finalize();

    kprintln!("[tls] sh_hash[0..4]={:02x}{:02x}{:02x}{:02x} ch_len={} sh_len={}",
        sh_hash[0], sh_hash[1], sh_hash[2], sh_hash[3],
        client_hello.len(), server_hello.len());
    // Debug: verify ClientHello starts with 0x01 (ClientHello type)
    kprintln!("[tls] CH[0..4]={:02x}{:02x}{:02x}{:02x} SH[0..4]={:02x}{:02x}{:02x}{:02x}",
        client_hello[0], client_hello[1], client_hello[2], client_hello[3],
        server_hello[0], server_hello[1], server_hello[2], server_hello[3]);

    // Client/Server Handshake Traffic Secrets
    let client_hs_secret = hmac::derive_secret(&handshake_secret, b"c hs traffic", &sh_hash);
    let server_hs_secret = hmac::derive_secret(&handshake_secret, b"s hs traffic", &sh_hash);
    kprintln!("[tls] server_hs_secret[0..4]={:02x}{:02x}{:02x}{:02x}",
        server_hs_secret[0], server_hs_secret[1], server_hs_secret[2], server_hs_secret[3]);

    // Handshake keys
    let server_hs_key = expand_to_key(&server_hs_secret);
    let server_hs_iv = expand_to_iv(&server_hs_secret);
    let client_hs_key = expand_to_key(&client_hs_secret);
    let client_hs_iv = expand_to_iv(&client_hs_secret);

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

        kprintln!("[tls] Record: ct=0x{:02x} len={}", ct, record.len());

        if ct == CT_CHANGE_CIPHER_SPEC {
            kprintln!("[tls] Skipping CCS");
            continue; // Ignore legacy CCS
        }

        if ct != CT_APPLICATION_DATA {
            kprintln!("[tls] Unexpected ct, first bytes: {:02x} {:02x}",
                record.first().copied().unwrap_or(0),
                record.get(1).copied().unwrap_or(0));
            return Err(TlsError::UnexpectedMessage);
        }

        // Decrypt handshake record
        let nonce = build_nonce(&server_hs_iv, server_hs_seq);
        server_hs_seq += 1;

        // AAD is the record header (content type + version + length of encrypted payload)
        let aad = build_record_aad(CT_APPLICATION_DATA, record.len());
        kprintln!("[tls] Decrypt: seq={} aad={:02x}{:02x}{:02x}{:02x}{:02x}",
            server_hs_seq - 1, aad[0], aad[1], aad[2], aad[3], aad[4]);

        let plaintext = match crypto::aead_decrypt_aad(&server_hs_key, &nonce, &aad, &record) {
            Some(pt) => pt,
            None => {
                kprintln!("[tls] AEAD decrypt failed! key[0..4]={:02x}{:02x}{:02x}{:02x} iv[0..4]={:02x}{:02x}{:02x}{:02x}",
                    server_hs_key[0], server_hs_key[1], server_hs_key[2], server_hs_key[3],
                    server_hs_iv[0], server_hs_iv[1], server_hs_iv[2], server_hs_iv[3]);
                return Err(TlsError::DecryptError);
            }
        };

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
    // Clone transcript state before finalizing (needed for app key derivation)
    let transcript_before_sf = transcript.clone();
    let hs_hash = transcript.finalize();

    let finished_key = expand_finished_key(&server_hs_secret);
    let expected_finished = hmac::hmac_sha256(&finished_key, &hs_hash);
    if server_finished.len() != 32 || !constant_time_eq(&server_finished, &expected_finished) {
        return Err(TlsError::HandshakeFailed("finished verify failed"));
    }

    // === Send Client Finished ===
    // Client Finished verify_data uses transcript including server Finished
    let mut cf_transcript = transcript_before_sf.clone();
    let mut sf_hs_msg = Vec::new();
    sf_hs_msg.push(HT_FINISHED);
    sf_hs_msg.push(0); sf_hs_msg.push(0); sf_hs_msg.push(server_finished.len() as u8);
    sf_hs_msg.extend_from_slice(&server_finished);
    cf_transcript.update(&sf_hs_msg);
    let cf_hash = cf_transcript.finalize();

    let client_finished_key = expand_finished_key(&client_hs_secret);
    let client_finished_data = hmac::hmac_sha256(&client_finished_key, &cf_hash);

    // Build the Finished handshake message
    let mut finished_msg = Vec::new();
    finished_msg.push(HT_FINISHED);
    finished_msg.push(0); finished_msg.push(0); finished_msg.push(32);
    finished_msg.extend_from_slice(&client_finished_data);

    // Encrypt and send Client Finished
    let client_nonce = build_nonce(&client_hs_iv, 0);
    let mut inner_with_ct = finished_msg.clone();
    inner_with_ct.push(CT_HANDSHAKE);
    let aad = build_record_aad(CT_APPLICATION_DATA, inner_with_ct.len() + 16);
    let encrypted = crypto::aead_encrypt_aad(&client_hs_key, &client_nonce, &aad, &inner_with_ct);
    send_record(tcp_handle, CT_APPLICATION_DATA, &encrypted)?;

    // === Derive Application Keys ===
    // App traffic secrets use Hash(CH..SF) — transcript including server Finished
    // Clone transcript (which is at CH..CV state), add SF, then finalize
    let mut app_transcript = transcript_before_sf.clone();
    // Add the server Finished message to transcript
    let mut sf_msg = Vec::new();
    sf_msg.push(HT_FINISHED);
    sf_msg.push(0); sf_msg.push(0); sf_msg.push(server_finished.len() as u8);
    sf_msg.extend_from_slice(&server_finished);
    app_transcript.update(&sf_msg);
    let app_hash = app_transcript.finalize();

    let derived2 = hmac::derive_secret(&handshake_secret, b"derived", &empty_hash);
    let master_secret = hmac::hkdf_extract(&derived2, &[0u8; 32]);

    let client_app_secret = hmac::derive_secret(&master_secret, b"c ap traffic", &app_hash);
    let server_app_secret = hmac::derive_secret(&master_secret, b"s ap traffic", &app_hash);

    let client_app_key = expand_to_key(&client_app_secret);
    let server_app_key = expand_to_key(&server_app_secret);
    let client_app_iv = expand_to_iv(&client_app_secret);
    let server_app_iv = expand_to_iv(&server_app_secret);

    Ok(TlsSession {
        tcp_handle,
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

    let aad = build_record_aad(CT_APPLICATION_DATA, inner.len() + 16);
    let encrypted = crypto::aead_encrypt_aad(&session.client_app_key, &nonce, &aad, &inner);
    send_record(session.tcp_handle, CT_APPLICATION_DATA, &encrypted)?;
    Ok(())
}

/// Receive application data over TLS.
pub fn tls_recv(session: &mut TlsSession, buf: &mut [u8]) -> Result<usize, TlsError> {
    let (ct, record) = recv_record(session.tcp_handle)?;

    kprintln!("[tls] app_recv: ct=0x{:02x} len={} seq={}", ct, record.len(), session.server_seq);

    if ct == CT_CHANGE_CIPHER_SPEC {
        return Ok(0);
    }
    if ct != CT_APPLICATION_DATA {
        return Err(TlsError::UnexpectedMessage);
    }

    let nonce = build_nonce(&session.server_app_iv, session.server_seq);
    session.server_seq += 1;

    let aad = build_record_aad(CT_APPLICATION_DATA, record.len());
    let plaintext = match crypto::aead_decrypt_aad(&session.server_app_key, &nonce, &aad, &record) {
        Some(pt) => pt,
        None => {
            kprintln!("[tls] app decrypt FAILED");
            return Err(TlsError::DecryptError);
        }
    };

    if plaintext.is_empty() {
        return Ok(0);
    }

    let real_ct = plaintext[plaintext.len() - 1];
    let data = &plaintext[..plaintext.len() - 1];

    kprintln!("[tls] app inner_ct=0x{:02x} data_len={}", real_ct, data.len());

    if real_ct == CT_ALERT {
        if data.len() >= 2 {
            kprintln!("[tls] Alert: level={} desc={}", data[0], data[1]);
        }
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

    let aad = build_record_aad(CT_APPLICATION_DATA, alert.len() + 16);
    let encrypted = crypto::aead_encrypt_aad(&session.client_app_key, &nonce, &aad, &alert);
    let _ = send_record(session.tcp_handle, CT_APPLICATION_DATA, &encrypted);
    let _ = tcp::close(session.tcp_handle);
    Ok(())
}

// ============================================================
// Internal helpers
// ============================================================

fn build_client_hello(random: &[u8; 32], pubkey: &[u8; 32], hostname: &str) -> Vec<u8> {
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

    // Supported Groups: x25519
    put_u16(&mut extensions, EXT_SUPPORTED_GROUPS);
    put_u16(&mut extensions, 4);
    put_u16(&mut extensions, 2);
    put_u16(&mut extensions, GROUP_X25519);

    // Key Share: x25519 public key
    // Entry: group(2) + key_len(2) + key(32) = 36
    put_u16(&mut extensions, EXT_KEY_SHARE);
    put_u16(&mut extensions, 38); // extension data: shares_len(2) + 36
    put_u16(&mut extensions, 36); // client_shares length
    put_u16(&mut extensions, GROUP_X25519);
    put_u16(&mut extensions, 32);
    extensions.extend_from_slice(pubkey);

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

    // Cipher suites
    put_u16(&mut body, 2); // Length
    body.extend_from_slice(&CIPHER_SUITE);

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

fn parse_server_hello(msg: &[u8]) -> Result<[u8; 32], TlsError> {
    // Skip: type(1) + length(3) + version(2) + random(32) + session_id_len(1) + session_id + cipher(2) + compression(1)
    if msg.len() < 4 { return Err(TlsError::HandshakeFailed("ServerHello too short")); }
    let mut pos = 4; // Skip handshake header

    if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no version")); }
    pos += 2; // version

    if pos + 32 > msg.len() { return Err(TlsError::HandshakeFailed("no random")); }
    pos += 32; // server random

    if pos >= msg.len() { return Err(TlsError::HandshakeFailed("no session id len")); }
    let sid_len = msg[pos] as usize;
    pos += 1 + sid_len;

    pos += 2; // cipher suite
    pos += 1; // compression

    // Extensions
    if pos + 2 > msg.len() { return Err(TlsError::HandshakeFailed("no extensions")); }
    let ext_len = ((msg[pos] as usize) << 8) | msg[pos + 1] as usize;
    pos += 2;

    let ext_end = pos + ext_len;
    let mut server_pubkey = [0u8; 32];
    let mut found_key = false;

    while pos + 4 <= ext_end && pos + 4 <= msg.len() {
        let ext_type = ((msg[pos] as u16) << 8) | msg[pos + 1] as u16;
        let data_len = ((msg[pos + 2] as usize) << 8) | msg[pos + 3] as usize;
        pos += 4;

        if ext_type == EXT_KEY_SHARE && data_len >= 36 {
            // group(2) + key_len(2) + key(32)
            let key_start = pos + 4;
            if key_start + 32 <= msg.len() {
                server_pubkey.copy_from_slice(&msg[key_start..key_start + 32]);
                found_key = true;
            }
        }

        pos += data_len;
    }

    if !found_key {
        return Err(TlsError::HandshakeFailed("no server key_share"));
    }

    Ok(server_pubkey)
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
        kprintln!("[tls] Expected handshake (0x16), got content_type=0x{:02x}, len={}", ct, payload.len());
        if !payload.is_empty() {
            kprintln!("[tls] First bytes: {:02x} {:02x} {:02x} {:02x}",
                payload[0], payload.get(1).copied().unwrap_or(0),
                payload.get(2).copied().unwrap_or(0), payload.get(3).copied().unwrap_or(0));
        }
        return Err(TlsError::UnexpectedMessage);
    }
    if payload.is_empty() || payload[0] != expected_type {
        kprintln!("[tls] Expected hs_type=0x{:02x}, got 0x{:02x}", expected_type,
            payload.first().copied().unwrap_or(0));
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

fn expand_to_key(secret: &[u8; 32]) -> [u8; 32] {
    let expanded = hmac::hkdf_expand_label(secret, b"key", &[], 32);
    let mut key = [0u8; 32];
    key.copy_from_slice(&expanded);
    key
}

fn expand_to_iv(secret: &[u8; 32]) -> [u8; 12] {
    let expanded = hmac::hkdf_expand_label(secret, b"iv", &[], 12);
    let mut iv = [0u8; 12];
    iv.copy_from_slice(&expanded);
    iv
}

fn expand_finished_key(secret: &[u8; 32]) -> [u8; 32] {
    let expanded = hmac::hkdf_expand_label(secret, b"finished", &[], 32);
    let mut key = [0u8; 32];
    key.copy_from_slice(&expanded);
    key
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
