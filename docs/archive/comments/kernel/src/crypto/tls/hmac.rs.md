# `kernel/src/crypto/tls/hmac.rs` @ 5e0102684

## L1-4 · `use alloc::vec::Vec;`

```
//! HMAC and HKDF — wrappers around `hmac` and `hkdf` crates (RFC 5869, audited)
//!
//! SHA-256 variants for TLS_AES_128_GCM_SHA256 and TLS_CHACHA20_POLY1305_SHA256.
//! SHA-384 variants for TLS_AES_256_GCM_SHA384.
```

## L13-15 · `pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {`

```
// ============================================================
// SHA-256 variants
// ============================================================
```

## L17 · `pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {`

```
/// HMAC-SHA256
```

## L27 · `pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {`

```
/// HKDF-Extract (RFC 5869 Section 2.2)
```

## L33 · `pub fn hkdf_expand(prk: &[u8; 32], info: &[u8], length: usize) -> Vec<u8> {`

```
/// HKDF-Expand (RFC 5869 Section 2.3)
```

## L41 · `pub fn hkdf_expand_label(`

```
/// HKDF-Expand-Label for TLS 1.3 (RFC 8446 Section 7.1)
```

## L61 · `pub fn derive_secret(secret: &[u8; 32], label: &[u8], transcript_hash: &[u8; 32]) -> [u8; 32] {`

```
/// Derive-Secret for TLS 1.3 key schedule
```

## L69-71 · `pub fn hmac_sha384(key: &[u8], message: &[u8]) -> [u8; 48] {`

```
// ============================================================
// SHA-384 variants (for TLS_AES_256_GCM_SHA384)
// ============================================================
```

## L73 · `pub fn hmac_sha384(key: &[u8], message: &[u8]) -> [u8; 48] {`

```
/// HMAC-SHA384
```

## L83 · `pub fn hkdf_extract_384(salt: &[u8], ikm: &[u8]) -> [u8; 48] {`

```
/// HKDF-Extract with SHA-384
```

## L89 · `pub fn hkdf_expand_384(prk: &[u8; 48], info: &[u8], length: usize) -> Vec<u8> {`

```
/// HKDF-Expand with SHA-384
```

## L97 · `pub fn hkdf_expand_label_384(`

```
/// HKDF-Expand-Label with SHA-384 for TLS 1.3
```

## L117 · `pub fn derive_secret_384(secret: &[u8; 48], label: &[u8], transcript_hash: &[u8; 48]) -> [u8; 48] {`

```
/// Derive-Secret with SHA-384 for TLS 1.3 key schedule
```

