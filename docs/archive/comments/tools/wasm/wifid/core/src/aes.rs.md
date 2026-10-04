# `tools/wasm/wifid/core/src/aes.rs` @ 5e0102684

## L1-7 · `const SBOX: [u8; 256] = [`

```
//! AES-128 (FIPS-197) + AES Key Unwrap (RFC 3394).
//!
//! WPA2 msg3 carries the GTK in `key_data`, AES-key-wrapped with the KEK (the
//! second 16 bytes of the PTK). We only need encrypt (the unwrap KDF runs the
//! cipher in the *decrypt* direction, so we also need the inverse cipher).
//! Small table-free implementation — correctness over speed (a handshake runs a
//! handful of blocks).
```

## L9 · `const SBOX: [u8; 256] = [`

```
// ── S-box / inverse S-box (FIPS-197) ─────────────────────────────────────
```

## L30 · `SBOX.iter().position(|&x| x == b).unwrap() as u8`

```
// Derived from SBOX (avoids a second 256-byte table).
```

## L55 · `pub struct Aes128 {`

```
/// Expanded AES-128 key schedule (11 round keys × 16 bytes).
```

## L131 · `fn shift_rows(s: &mut [u8; 16]) {`

```
// State is column-major (AES standard): s[r + 4c].
```

## L167-170 · `pub fn aes_wrap(kek: &[u8; 16], key: &[u8], out: &mut [u8]) -> bool {`

```
// ── AES Key Wrap (RFC 3394) — inverse of unwrap ───────────────────────────
// Wraps `key` (n×8 bytes, n≥2) into `out` ((n+1)×8 bytes). Only used by the
// dev-harness to build a synthetic msg3 for the self-consistency test; the
// supplicant itself only ever unwraps.
```

## L199-202 · `pub fn aes_unwrap(kek: &[u8; 16], wrapped: &[u8], out: &mut [u8]) -> bool {`

```
// ── AES Key Unwrap (RFC 3394) ─────────────────────────────────────────────
// Unwrap `n` 64-bit blocks (ciphertext is (n+1)*8 bytes). Returns the unwrapped
// key (n*8 bytes) into `out`, true if the integrity check (A == 0xA6A6A6A6A6A6A6A6)
// passes. Used to decrypt msg3's key_data with the KEK.
```

## L214 · `let mut r = [0u8; 8 * 64]; // up to 64 blocks`

```
// up to 64 blocks
```

## L220 · `let t = (n * j + i) as u64;`

```
// B = AES-decrypt( (A ^ t) || R[i] ), t = n*j + i
```

