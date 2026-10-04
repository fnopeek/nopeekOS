# `kernel/src/crypto/aead.rs` @ 5e0102684

## L1-8 · `use alloc::vec::Vec;`

```
//! Cryptographic primitives for nopeekOS
//!
//! - ChaCha20 stream cipher (RFC 7539)
//! - Poly1305 MAC (RFC 7539)
//! - ChaCha20-Poly1305 AEAD (RFC 8439)
//! - BLAKE3-based KDF for key derivation
//!
//! All implementations target no_std bare metal.
```

## L13-15 · `static MASTER_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);`

```
// ============================================================
// Global Master Key (set after passphrase auth)
// ============================================================
```

## L31-33 · `fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {`

```
// ============================================================
// ChaCha20 (RFC 7539)
// ============================================================
```

## L74 · `fn chacha20_xor(key: &[u8; 32], nonce: &[u8; 12], counter: u32, data: &mut [u8]) {`

```
/// XOR data with ChaCha20 keystream. Works for both encrypt and decrypt.
```

## L89-94 · `fn poly1305_mac(key: &[u8; 32], message: &[u8]) -> [u8; 16] {`

```
// ============================================================
// Poly1305 MAC (RFC 7539)
// ============================================================
//
// Uses u128 accumulator with explicit 130-bit representation (u128 + u8).
// Product computed via 64-bit limb schoolbook multiply into u128 intermediates.
```

## L97 · `let mut r_bytes = [0u8; 16];`

```
// Clamp r
```

## L109 · `let mut acc_lo: u128 = 0;`

```
// Accumulator: (acc_lo: u128, acc_hi: u8) = 130-bit number
```

## L118 · `let (sum, carry1) = acc_lo.overflowing_add(n);`

```
// acc += n + 2^128 (hibit)
```

## L122 · `acc_hi += 1; // Add the 2^128 hibit`

```
// Add the 2^128 hibit
```

## L124 · `poly1305_mulmod(&mut acc_lo, &mut acc_hi, r);`

```
// acc = acc * r mod (2^130 - 5)
```

## L151 · `poly1305_reduce(&mut acc_lo, &mut acc_hi);`

```
// Final reduction mod p
```

## L154 · `let tag = acc_lo.wrapping_add(s);`

```
// tag = (acc + s) mod 2^128
```

## L159 · `#[allow(unused_variables, unused_mut, unused_assignments)]`

```
/// Multiply 130-bit accumulator by 128-bit r, reduce mod 2^130-5
```

## L162 · `let a0 = *acc_lo as u64;`

```
// Split into 64-bit limbs for multiplication
```

## L165 · `let a2 = *acc_hi as u64; // 0-3`

```
// 0-3
```

## L170 · `let m00 = a0 as u128 * r0 as u128;`

```
// Schoolbook: (a0 + a1*2^64 + a2*2^128) * (r0 + r1*2^64)
```

## L178 · `let d0 = m00;`

```
// Combine: result = d[0] + d[1]*2^64 + d[2]*2^128 + d[3]*2^192
```

## L180 · `let d1 = m01 + m10;   // might overflow u128 but won't in practice (64*64+64*64 < 2^129)`

```
// might overflow u128 but won't in practice (64*64+64*64 < 2^129)
```

## L185 · `let mut w0 = d0 as u64;`

```
// Assemble 256-bit number as 4 x 64-bit
```

## L191 · `let (t, c) = (w1 as u128 + (d1 as u64) as u128).overflowing_add(0); // can't overflow u128`

```
// can't overflow u128
```

## L205-206 · `let lo = w0 as u128 | ((w1 as u128) << 64);`

```
// 320-bit result in w0..w4
// Reduce mod 2^130 - 5: low 130 bits + (above >> 130) * 5
```

## L208 · `let lo_130 = lo & ((1u128 << 127) - 1 + (1u128 << 127)); // all 128 bits`

```
// all 128 bits
```

## L211 · `let above = (w2 >> 2) as u128 | ((w3 as u128) << 62) | ((w4 as u128) << 126);`

```
// above 130 = w2>>2 | w3<<62 | w4<<126
```

## L214 · `let mul5 = above.wrapping_mul(5);`

```
// result = lo_128 + bits_128_129 * 2^128 + above * 5
```

## L220 · `if new_hi >= 4 {`

```
// If new_hi >= 4, reduce again
```

## L233 · `fn poly1305_reduce(acc_lo: &mut u128, acc_hi: &mut u8) {`

```
/// Final reduction: ensure acc < 2^130 - 5
```

## L235-237 · `let (test, c) = acc_lo.overflowing_add(5);`

```
// If acc >= p, subtract p
// p = 2^130 - 5
// Test: acc + 5 >= 2^130?
```

## L241 · `if *acc_hi >= 4 { *acc_hi &= 3; }`

```
// acc >= p, use the reduced value
```

## L244 · `if *acc_hi >= 4 { *acc_hi &= 3; }`

```
// If still >= p after one reduction, do again (shouldn't happen)
```

## L247-249 · `}`

```
// Now acc < p. Take bottom 128 bits for final output.
// acc_hi has bits 128-129, but for the tag we only need bits 0-127
// (the final tag computation is (h + s) mod 2^128)
```

## L252-254 · `pub const TAG_SIZE: usize = 16;`

```
// ============================================================
// ChaCha20-Poly1305 AEAD (RFC 8439)
// ============================================================
```

## L258-264 · `use aes_gcm::aead::{Aead, AeadInOut, KeyInit};`

```
// AES-256-GCM via the `aes-gcm` crate. The crate's `aes` backend
// auto-detects AES-NI at runtime via `cpufeatures` and falls back to
// constant-time soft AES on CPUs without it. N100 has AES-NI, so the
// fast path is taken in practice.
//
// File-system encryption uses AES-GCM; TLS keeps ChaCha20-Poly1305 for
// cipher-suite compatibility with peers that negotiate it.
```

## L269-270 · `pub fn aead_encrypt_aes(key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8]) -> Vec<u8> {`

```
/// Encrypt `plaintext` with AES-256-GCM. Returns ciphertext || 16-byte tag.
/// Hardware-accelerated when AES-NI is present.
```

## L277-280 · `pub fn aead_decrypt_aes_in_place(`

```
/// Decrypt `buf` (ciphertext || 16-byte tag) in place. On success `buf`
/// is shrunk to plaintext-only and `Some(())` is returned; on tag
/// mismatch / short input, `buf` is left untouched and `None` is
/// returned. Saves one Vec alloc + one full-payload memcpy.
```

## L301-304 · `pub fn aead_encrypt_aad(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {`

```
/// Encrypt and authenticate with Additional Authenticated Data (AAD).
/// ChaCha20-Poly1305. Used by TLS record layer when the negotiated
/// cipher suite is `TLS_CHACHA20_POLY1305_SHA256`. File-system
/// encryption uses AES-GCM via [`aead_encrypt_aes`] instead.
```

## L320-321 · `pub fn aead_decrypt_aad(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ciphertext_and_tag: &[u8]) -> Option<Vec<u8>> {`

```
/// Decrypt and verify with AAD. ChaCha20-Poly1305 — TLS counterpart
/// to [`aead_encrypt_aad`]. Storage path uses [`aead_decrypt_aes`].
```

## L338 · `let mut diff = 0u8;`

```
// Constant-time comparison
```

## L365-367 · `pub fn derive_master_key(passphrase: &[u8], salt: &[u8]) -> [u8; 32] {`

```
// ============================================================
// Key Derivation (BLAKE3-based)
// ============================================================
```

## L369 · `pub fn derive_master_key(passphrase: &[u8], salt: &[u8]) -> [u8; 32] {`

```
/// Derive a 256-bit master key from a passphrase and salt.
```

## L378 · `pub fn derive_object_key(master_key: &[u8; 32], content_hash: &[u8; 32]) -> [u8; 32] {`

```
/// Derive a per-object encryption key from master key and object content hash.
```

## L386 · `pub fn derive_nonce(content_hash: &[u8; 32]) -> [u8; 12] {`

```
/// Derive a nonce from the content hash (first 12 bytes).
```

