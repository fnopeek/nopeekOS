//! The disk's data key: created at setup, unlocked at login, re-wrapped when
//! the passphrase changes. See `crypto::keyslot` for the slot format and
//! `crypto::aead` for the keys derived from it.

use crate::crypto::{self, keyslot};
use crate::kprintln;

/// Keyslot that holds the passphrase.
const PASSPHRASE_SLOT: usize = 0;

/// The DEK, if `pass` opens one of the disk's keyslots. Each attempt costs
/// a full Argon2id run; that cost is the point.
fn open(pass: &[u8]) -> Option<[u8; 32]> {
    for i in 0..crate::npkfs::KEYSLOTS {
        let Some(slot) = crate::npkfs::keyslot(i) else { continue };
        if keyslot::is_empty(&slot) { continue; }
        if let Some(dek) = keyslot::unlock(&slot, pass) {
            return Some(dek);
        }
    }
    None
}

/// Unlock the disk with `pass`. False for a wrong passphrase.
pub fn unlock(pass: &[u8]) -> bool {
    match open(pass) {
        Some(mut dek) => {
            crypto::set_disk_key(&dek);
            dek.fill(0);
            true
        }
        None => false,
    }
}

/// Lock: the derived keys leave memory; the next access needs `unlock`.
pub fn lock() {
    crypto::clear_disk_key();
}

/// Fresh install: draw a new DEK, wrap it under `pass` and unlock.
pub fn create(pass: &[u8]) -> Result<(), &'static str> {
    crate::csprng::reseed();
    if !crate::csprng::hardware_seeded() {
        kprintln!("[npk]   WARNING: no hardware RNG passed its check — the disk key");
        kprintln!("[npk]   rests on timing jitter alone.");
    }
    let mut dek = crate::csprng::random_256();
    kprintln!("[npk]   Measuring key derivation...");
    let params = keyslot::calibrate();
    kprintln!("[npk]   Argon2id: {} MiB, {} pass(es), {} lanes",
        params.m_kib / 1024, params.t, params.p);
    let slot = keyslot::create(pass, &dek, params);
    let r = match slot {
        Some(slot) => crate::npkfs::set_keyslot(PASSPHRASE_SLOT, &slot)
            .map_err(|_| "could not write the keyslot"),
        None => Err("not enough memory for the key derivation"),
    };
    if r.is_ok() {
        crypto::set_disk_key(&dek);
    }
    dek.fill(0);
    r
}

/// Replace the passphrase: `old` must open a slot; the same DEK is wrapped
/// under `new`, keeping the slot's cost. The data is not touched.
pub fn change(old: &[u8], new: &[u8]) -> Result<(), &'static str> {
    let mut dek = open(old).ok_or("wrong passphrase")?;
    let params = crate::npkfs::keyslot(PASSPHRASE_SLOT)
        .filter(|s| !keyslot::is_empty(s))
        .map(|s| keyslot::params_of(&s))
        .unwrap_or_else(keyslot::calibrate);
    let r = keyslot::create(new, &dek, params)
        .ok_or("not enough memory for the key derivation")
        .and_then(|slot| crate::npkfs::set_keyslot(PASSPHRASE_SLOT, &slot)
            .map_err(|_| "could not write the keyslot"));
    dek.fill(0);
    r
}
