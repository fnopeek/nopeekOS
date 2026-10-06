//! Disk keyslots: the passphrase unwraps the data key, it is not the key.
//!
//! The data encryption key (DEK) is 32 random bytes drawn once at install.
//! A keyslot holds it wrapped with AES-256-GCM under a key-encryption key
//! derived from the passphrase with Argon2id (RFC 9106). Changing the
//! passphrase rewrites a slot; the data stays as it is. The Argon2 parameters
//! live in the slot, so they can be raised later the same way.
//!
//! Slot layout (`SLOT_BYTES`), little-endian:
//!
//! ```text
//!   0      kind (0 = empty, 1 = passphrase, Argon2id v0x13)
//!   1..4   reserved, zero
//!   4..8   memory in KiB
//!   8..12  passes (t)
//!   12..16 lanes (p)
//!   16..32 salt
//!   32..44 GCM nonce
//!   44..92 wrapped DEK (32) + tag (16)
//! ```
//!
//! Bytes 0..32 are the GCM associated data: the parameters and salt cannot
//! be altered without the unwrap failing. A failed unwrap is also how a
//! wrong passphrase shows; there is no stored known plaintext to test
//! guesses against.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};

pub const SLOT_BYTES: usize = 128;
pub type Slot = [u8; SLOT_BYTES];

const KIND_PASSPHRASE: u8 = 1;
const AAD_END: usize = 32;
const WRAPPED: core::ops::Range<usize> = 44..92;

/// Argon2id cost. Lanes are computed one after another here (no threads in
/// the KDF), so `p` costs us nothing extra and only fixes the algorithm's
/// shape at the RFC 9106 value.
#[derive(Clone, Copy)]
pub struct Params {
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

/// Lanes, as in both RFC 9106 profiles.
const LANES: u32 = 4;
/// Memory ceiling: 1 GiB, the upper end cryptsetup benchmarks into.
const MAX_MIB: usize = 1024;
/// Memory floor: RFC 9106's low-memory profile.
const MIN_MIB: usize = 64;
/// What an unlock may take on this machine.
const TARGET_MS: u64 = 1000;
const MAX_PASSES: u32 = 8;

pub fn is_empty(slot: &Slot) -> bool {
    slot[0] == 0
}

/// Argon2id over `pass` and `salt`, its working memory taken from physical
/// frames and wiped before they go back. None if the memory is not there.
fn argon2id(pass: &[u8], salt: &[u8; 16], p: Params) -> Option<[u8; 32]> {
    let params = argon2::Params::new(p.m_kib, p.t, p.p, Some(32)).ok()?;
    let a = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let blocks = a.params().block_count();
    let bytes = blocks * core::mem::size_of::<argon2::Block>();
    let pages = bytes.div_ceil(crate::memory::PAGE_SIZE);
    let Some(base) = crate::memory::allocate_contiguous(pages) else {
        // Otherwise this would look like a wrong passphrase.
        crate::kprintln!("[npk] key derivation needs {} MiB of contiguous memory — not available",
            pages * crate::memory::PAGE_SIZE / (1024 * 1024));
        return None;
    };
    // SAFETY: `pages` fresh, identity-mapped frames owned by this function
    // until they are returned below; page alignment satisfies Block's 64-byte
    // alignment, and every bit pattern is a valid Block (`[u64; 128]`).
    let mem = unsafe { core::slice::from_raw_parts_mut(base as *mut argon2::Block, blocks) };
    let mut out = [0u8; 32];
    let r = a.hash_password_into_with_memory(pass, salt, &mut out, &mut *mem);
    // SAFETY: same frames, still owned; the passphrase-derived state must
    // not survive into whatever gets these frames next.
    unsafe { core::ptr::write_bytes(base as *mut u8, 0, pages * crate::memory::PAGE_SIZE) };
    crate::memory::deallocate_contiguous(base, pages);
    r.ok().map(|_| out)
}

/// Pick parameters for this machine: as much memory as RAM comfortably
/// allows (64 MiB .. 1 GiB), then as many passes as fit in about a second.
pub fn calibrate() -> Params {
    let (_, free_mib) = crate::memory::stats();
    let mut mib = MIN_MIB;
    while mib * 2 <= MAX_MIB && mib * 2 <= free_mib / 4 {
        mib *= 2;
    }
    let one = Params { m_kib: (mib * 1024) as u32, t: 1, p: LANES };
    let freq = crate::interrupts::tsc_freq().max(1);
    let t0 = crate::interrupts::rdtsc();
    let _ = argon2id(b"calibration", &[0u8; 16], one);
    let ms = (crate::interrupts::rdtsc().saturating_sub(t0) * 1000 / freq).max(1);
    let t = ((TARGET_MS + ms / 2) / ms).clamp(1, MAX_PASSES as u64) as u32;
    Params { t, ..one }
}

/// A passphrase slot for `dek`, with fresh salt and nonce.
pub fn create(pass: &[u8], dek: &[u8; 32], params: Params) -> Option<Slot> {
    let mut slot = [0u8; SLOT_BYTES];
    slot[0] = KIND_PASSPHRASE;
    slot[4..8].copy_from_slice(&params.m_kib.to_le_bytes());
    slot[8..12].copy_from_slice(&params.t.to_le_bytes());
    slot[12..16].copy_from_slice(&params.p.to_le_bytes());
    let rnd = crate::csprng::random_256();
    slot[16..32].copy_from_slice(&rnd[..16]);
    slot[32..44].copy_from_slice(&rnd[16..28]);

    let salt: [u8; 16] = slot[16..32].try_into().ok()?;
    let kek = argon2id(pass, &salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(&kek).ok()?;
    let nonce = Nonce::from(<[u8; 12]>::try_from(&slot[32..44]).ok()?);
    let ct = cipher.encrypt(&nonce, Payload { msg: dek, aad: &slot[..AAD_END] }).ok()?;
    if ct.len() != WRAPPED.len() {
        return None;
    }
    slot[WRAPPED].copy_from_slice(&ct);
    Some(slot)
}

/// The DEK, if `pass` opens this slot.
pub fn unlock(slot: &Slot, pass: &[u8]) -> Option<[u8; 32]> {
    if slot[0] != KIND_PASSPHRASE {
        return None;
    }
    let u32_at = |i: usize| u32::from_le_bytes([slot[i], slot[i + 1], slot[i + 2], slot[i + 3]]);
    let params = Params { m_kib: u32_at(4), t: u32_at(8), p: u32_at(12) };
    let salt: [u8; 16] = slot[16..32].try_into().ok()?;
    let kek = argon2id(pass, &salt, params)?;
    let cipher = Aes256Gcm::new_from_slice(&kek).ok()?;
    let nonce = Nonce::from(<[u8; 12]>::try_from(&slot[32..44]).ok()?);
    let pt = cipher.decrypt(&nonce, Payload { msg: &slot[WRAPPED], aad: &slot[..AAD_END] }).ok()?;
    <[u8; 32]>::try_from(pt.as_slice()).ok()
}

/// Memory and passes of a slot, for display.
pub fn params_of(slot: &Slot) -> Params {
    let u32_at = |i: usize| u32::from_le_bytes([slot[i], slot[i + 1], slot[i + 2], slot[i + 3]]);
    Params { m_kib: u32_at(4), t: u32_at(8), p: u32_at(12) }
}
