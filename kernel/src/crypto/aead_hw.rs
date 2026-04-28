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

use aes::Aes256;
use aes::cipher::{KeyInit, BlockEncrypt, KeyIvInit, StreamCipher};
use ctr::Ctr32BE;
use ghash::GHash;
use ghash::universal_hash::{KeyInit as UhKeyInit, UniversalHash};

use alloc::vec::Vec;

const TAG_LEN: usize = 16;
const BLOCK_LEN: usize = 16;
/// Chunk size for the interleaved CTR + GHASH single-pass loop.
/// Tuned to fit in L1d (Gracemont = 48 KB on N100) so both ops touch
/// the same cache lines without spilling. 4 KB = 256 blocks.
const CHUNK_BYTES: usize = 4096;
const CHUNK_BLOCKS: usize = CHUNK_BYTES / BLOCK_LEN;

type Aes256Ctr = Ctr32BE<Aes256>;

/// Build the (J0, GHASH-key, AES-cipher) triple from a key + 12-byte
/// nonce. AES-GCM with a 96-bit nonce derives J0 = nonce || 0x00000001
/// directly; longer nonces would need GHASH-derived J0, which we don't
/// support (and don't need — npkFS uses BLAKE3-derived 96-bit nonces).
fn setup(key: &[u8; 32], nonce: &[u8; 12]) -> (Aes256, [u8; BLOCK_LEN], GHash) {
    let cipher = Aes256::new(key.into());

    // H = E_K(0^128)
    let mut h = [0u8; BLOCK_LEN];
    cipher.encrypt_block((&mut h).into());
    let ghash = GHash::new(&h.into());

    // J0 = nonce || 0x00000001 — the AES-GCM standard's 96-bit nonce path
    let mut j0 = [0u8; BLOCK_LEN];
    j0[..12].copy_from_slice(nonce);
    j0[15] = 1;

    (cipher, j0, ghash)
}

/// `len_bits(AAD) || len_bits(C)` block, big-endian — the final input
/// to GHASH before tag derivation.
fn lengths_block(aad_len: usize, ct_len: usize) -> [u8; BLOCK_LEN] {
    let mut b = [0u8; BLOCK_LEN];
    b[..8].copy_from_slice(&((aad_len as u64) * 8).to_be_bytes());
    b[8..].copy_from_slice(&((ct_len as u64) * 8).to_be_bytes());
    b
}

fn ghash_padded(ghash: &mut GHash, data: &[u8]) {
    let nblocks = data.len() / BLOCK_LEN;
    if nblocks > 0 {
        let blocks = unsafe {
            core::slice::from_raw_parts(
                data.as_ptr() as *const ghash::Block, nblocks)
        };
        ghash.update(blocks);
    }
    let rem = data.len() % BLOCK_LEN;
    if rem != 0 {
        let mut last = [0u8; BLOCK_LEN];
        last[..rem].copy_from_slice(&data[nblocks * BLOCK_LEN..]);
        ghash.update(&[last.into()]);
    }
}

/// Encrypt `plaintext` with AES-256-GCM. Returns `ciphertext || tag`,
/// matching `aes_gcm::Aes256Gcm::encrypt(nonce, plaintext)` byte-for-byte.
///
/// Single-pass interleaved CTR + GHASH: each `CHUNK_BYTES` chunk gets
/// encrypted in place then immediately GHASHed before moving on. Both
/// ops touch the same cache lines so a 1 MB blob streams through L1
/// rather than reading the whole buffer twice from DRAM.
pub fn aead_encrypt_aes_hw(
    key: &[u8; 32], nonce: &[u8; 12], plaintext: &[u8],
) -> Vec<u8> {
    let (cipher, j0, mut ghash) = setup(key, nonce);

    let mut counter = j0;
    inc_counter(&mut counter);

    let mut buf = Vec::with_capacity(plaintext.len() + TAG_LEN);
    buf.extend_from_slice(plaintext);

    let mut ctr = Aes256Ctr::new(key.into(), (&counter).into());

    let pt_len = plaintext.len();
    let nfull = pt_len / CHUNK_BYTES;

    for i in 0..nfull {
        let off = i * CHUNK_BYTES;
        let chunk = &mut buf[off..off + CHUNK_BYTES];

        // Encrypt first (in-place), then GHASH the resulting CT.
        ctr.apply_keystream(chunk);
        let blocks = unsafe {
            core::slice::from_raw_parts(
                chunk.as_ptr() as *const ghash::Block, CHUNK_BLOCKS)
        };
        ghash.update(blocks);
    }

    let tail_off = nfull * CHUNK_BYTES;
    if tail_off < pt_len {
        let tail = &mut buf[tail_off..pt_len];
        ctr.apply_keystream(tail);
        ghash_padded(&mut ghash, tail);
    }

    let lens = lengths_block(0, pt_len);
    ghash.update(&[lens.into()]);
    let s = ghash.finalize();

    let mut t = j0;
    cipher.encrypt_block((&mut t).into());
    let mut tag = [0u8; TAG_LEN];
    for i in 0..TAG_LEN { tag[i] = t[i] ^ s[i]; }

    buf.extend_from_slice(&tag);
    buf
}

/// Decrypt `ciphertext_and_tag` in place. On success the buffer is
/// truncated to plaintext length and `Some(())` returned; on tag
/// mismatch the buffer is left untouched and `None` returned.
///
/// Single-pass interleaved GHASH + CTR: GHASH the CT chunk first
/// (so we hash unmodified ciphertext), then decrypt in place. Same
/// cache-line per chunk so the buffer streams through L1 once.
pub fn aead_decrypt_aes_hw_in_place(
    key: &[u8; 32], nonce: &[u8; 12], buf: &mut Vec<u8>,
) -> Option<()> {
    if buf.len() < TAG_LEN { return None; }
    let ct_len = buf.len() - TAG_LEN;

    let (cipher, j0, mut ghash) = setup(key, nonce);
    let mut counter = j0;
    inc_counter(&mut counter);
    let mut ctr = Aes256Ctr::new(key.into(), (&counter).into());

    let nfull = ct_len / CHUNK_BYTES;

    for i in 0..nfull {
        let off = i * CHUNK_BYTES;
        let chunk = &mut buf[off..off + CHUNK_BYTES];

        // GHASH the ciphertext (must come BEFORE decrypt — we
        // authenticate the on-the-wire bytes, not the plaintext).
        let blocks = unsafe {
            core::slice::from_raw_parts(
                chunk.as_ptr() as *const ghash::Block, CHUNK_BLOCKS)
        };
        ghash.update(blocks);

        // Decrypt in place — flips CT to PT.
        ctr.apply_keystream(chunk);
    }

    let tail_off = nfull * CHUNK_BYTES;
    if tail_off < ct_len {
        let tail = &mut buf[tail_off..ct_len];
        ghash_padded(&mut ghash, tail);
        ctr.apply_keystream(tail);
    }

    let lens = lengths_block(0, ct_len);
    ghash.update(&[lens.into()]);
    let s = ghash.finalize();

    // Tag verify (constant-time).
    let mut t = j0;
    cipher.encrypt_block((&mut t).into());
    let mut diff = 0u8;
    for i in 0..TAG_LEN {
        diff |= (t[i] ^ s[i]) ^ buf[ct_len + i];
    }
    if diff != 0 {
        // Tag mismatch — re-encrypt the buffer in place so we leave
        // the input untouched on failure (same recovery promise as
        // aes-gcm 0.10's decrypt_in_place_detached). Re-deriving the
        // counter is cheap; we throw away the CTR state.
        let mut counter2 = j0;
        inc_counter(&mut counter2);
        let mut ctr2 = Aes256Ctr::new(key.into(), (&counter2).into());
        ctr2.apply_keystream(&mut buf[..ct_len]);
        return None;
    }

    buf.truncate(ct_len);
    Some(())
}

fn inc_counter(c: &mut [u8; BLOCK_LEN]) {
    // 32-bit big-endian counter at the tail. Same convention as
    // AES-GCM (RFC 5288 §3) — only the low 4 bytes increment.
    let mut x = u32::from_be_bytes([c[12], c[13], c[14], c[15]]);
    x = x.wrapping_add(1);
    c[12..].copy_from_slice(&x.to_be_bytes());
}
