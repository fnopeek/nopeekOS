# `kernel/src/crypto/aead_hw.rs` @ 5e0102684

## L1-20 · `use aes::Aes256;`

```
//! AES-256-GCM, hand-glued from `aes` (AES-NI multi-block CTR) +
//! `ghash` (PCLMULQDQ single-block, replaced in v0.88.1 with a 4-way
//! aggregated implementation).
//!
//! Step 1 (v0.88.0): same backends as `aes-gcm 0.10` but with our own
//! AEAD glue. Goal: bit-for-bit identical output to `Aes256Gcm`. This
//! validates the framework without performance risk; once roundtrips
//! pass we can swap in custom GHASH for the actual win.
//!
//! Layout follows NIST SP 800-38D:
//!   1. Hash subkey  H = AES_K(0^128)
//!   2. Initial counter J0 = nonce || 0x00000001 (12-byte nonce path)
//!   3. Encrypt  C = AES-CTR_K(J0+1) ⊕ P
//!   4. Tag input S = GHASH_H(AAD || pad || C || pad ||
//!                            len(AAD) || len(C))
//!   5. Tag T = AES_K(J0) ⊕ S
//!
//! API parallels `crypto::aead`:
//!   - `aead_encrypt_aes_hw(&key, &nonce, plaintext)         -> Vec<u8>`
//!   - `aead_decrypt_aes_hw_in_place(&key, &nonce, &mut buf) -> Option<()>`
```

## L32-34 · `const CHUNK_BYTES: usize = 4096;`

```
/// Chunk size for the interleaved CTR + GHASH single-pass loop.
/// Tuned to fit in L1d (Gracemont = 48 KB on N100) so both ops touch
/// the same cache lines without spilling. 4 KB = 256 blocks.
```

## L40-43 · `fn setup(key: &[u8; 32], nonce: &[u8; 12]) -> (Aes256, [u8; BLOCK_LEN], GHash) {`

```
/// Build the (J0, GHASH-key, AES-cipher) triple from a key + 12-byte
/// nonce. AES-GCM with a 96-bit nonce derives J0 = nonce || 0x00000001
/// directly; longer nonces would need GHASH-derived J0, which we don't
/// support (and don't need — npkFS uses BLAKE3-derived 96-bit nonces).
```

## L47 · `let mut h = [0u8; BLOCK_LEN];`

```
// H = E_K(0^128)
```

## L52 · `let mut j0 = [0u8; BLOCK_LEN];`

```
// J0 = nonce || 0x00000001 — the AES-GCM standard's 96-bit nonce path
```

## L60-61 · `fn lengths_block(aad_len: usize, ct_len: usize) -> [u8; BLOCK_LEN] {`

```
/// `len_bits(AAD) || len_bits(C)` block, big-endian — the final input
/// to GHASH before tag derivation.
```

## L86-92 · `pub fn aead_encrypt_aes_hw(`

```
/// Encrypt `plaintext` with AES-256-GCM. Returns `ciphertext || tag`,
/// matching `aes_gcm::Aes256Gcm::encrypt(nonce, plaintext)` byte-for-byte.
///
/// Single-pass interleaved CTR + GHASH: each `CHUNK_BYTES` chunk gets
/// encrypted in place then immediately GHASHed before moving on. Both
/// ops touch the same cache lines so a 1 MB blob streams through L1
/// rather than reading the whole buffer twice from DRAM.
```

## L113 · `ctr.apply_keystream(chunk);`

```
// Encrypt first (in-place), then GHASH the resulting CT.
```

## L142-148 · `pub fn aead_decrypt_aes_hw_in_place(`

```
/// Decrypt `ciphertext_and_tag` in place. On success the buffer is
/// truncated to plaintext length and `Some(())` returned; on tag
/// mismatch the buffer is left untouched and `None` returned.
///
/// Single-pass interleaved GHASH + CTR: GHASH the CT chunk first
/// (so we hash unmodified ciphertext), then decrypt in place. Same
/// cache-line per chunk so the buffer streams through L1 once.
```

## L166-167 · `let blocks = unsafe {`

```
// GHASH the ciphertext (must come BEFORE decrypt — we
// authenticate the on-the-wire bytes, not the plaintext).
```

## L174 · `ctr.apply_keystream(chunk);`

```
// Decrypt in place — flips CT to PT.
```

## L189 · `let mut t = j0;`

```
// Tag verify (constant-time).
```

## L197-200 · `let mut counter2 = j0;`

```
// Tag mismatch — re-encrypt the buffer in place so we leave
// the input untouched on failure (same recovery promise as
// aes-gcm 0.10's decrypt_in_place_detached). Re-deriving the
// counter is cheap; we throw away the CTR state.
```

## L213-214 · `let mut x = u32::from_be_bytes([c[12], c[13], c[14], c[15]]);`

```
// 32-bit big-endian counter at the tail. Same convention as
// AES-GCM (RFC 5288 §3) — only the low 4 bytes increment.
```

