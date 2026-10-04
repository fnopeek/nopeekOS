# `tools/wasm/wifid/core/src/lib.rs` @ 5e0102684

## L1-12 · `#![no_std]`

```
//! wifid_core — vendor-independent WPA2 supplicant logic.
//!
//! `no_std` (+`alloc` not needed here) so the same code runs in `wifid.wasm`
//! and in the std dev-harness against published test vectors. This first slice
//! is the crypto foundation of WPA2-PSK:
//!
//! - SHA-1 + HMAC-SHA1 (FIPS 180/198)
//! - PBKDF2-HMAC-SHA1 → the 256-bit PMK from passphrase + SSID (IEEE 802.11i)
//! - PRF-SHA1 → the PTK from the PMK + nonces + MACs (the 4-way handshake KDF)
//!
//! The EAPOL 4-way state machine (MIC verify, GTK unwrap, key install) builds
//! on these and lands in the next slice.
```

## L19-20 · `pub struct Sha1 {`

```
// ── SHA-1 ────────────────────────────────────────────────────────────────
// FIPS 180-4. 64-byte block, 20-byte digest.
```

## L24 · `len: usize,    // bytes buffered in block`

```
// bytes buffered in `block`
```

## L25 · `total: u64,    // total message bytes`

```
// total message bytes
```

## L100 · `self.update(&[0x80]);`

```
// append 0x80, pad to 56 mod 64, then 64-bit big-endian length.
```

## L102 · `self.total -= 1; // the 0x80 isn't message data for length purposes`

```
// the 0x80 isn't message data for length purposes
```

## L123 · `pub fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {`

```
// ── HMAC-SHA1 (RFC 2104) ──────────────────────────────────────────────────
```

## L147-149 · `pub fn pbkdf2_sha1(passphrase: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {`

```
// ── PBKDF2-HMAC-SHA1 (RFC 2898) → PMK ─────────────────────────────────────
// WPA2: PMK = PBKDF2(passphrase, SSID, 4096, 32). Two 20-byte blocks → 40
// bytes, truncated to 32.
```

## L154 · `let mut msg = [0u8; 64];`

```
// U1 = HMAC(P, salt || INT(block_index))
```

## L174 · `pub fn wpa2_pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {`

```
/// WPA2-PSK PMK: 32 bytes from passphrase (8..63 ASCII) + SSID.
```

## L181-184 · `pub fn prf_sha1(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {`

```
// ── PRF-SHA1 (IEEE 802.11i) → PTK ─────────────────────────────────────────
// PTK = PRF(PMK, "Pairwise key expansion",
//          min(AA,SA) || max(AA,SA) || min(ANonce,SNonce) || max(ANonce,SNonce))
// CCMP PTK is 48 bytes (KCK16 || KEK16 || TK16).
```

## L189 · `let mut msg = [0u8; 128];`

```
// HMAC-SHA1(key, label || 0x00 || data || i)
```

## L208-209 · `pub fn wpa2_ptk(`

```
/// Derive the 48-byte CCMP PTK from the PMK, the two MACs and the two nonces.
/// `aa` = authenticator (AP) MAC, `sa` = supplicant (our) MAC.
```

## L218 · `if aa[..] < sa[..] {`

```
// min(AA,SA) || max(AA,SA)
```

## L226 · `if anonce[..] < snonce[..] {`

```
// min(ANonce,SNonce) || max(ANonce,SNonce)
```

