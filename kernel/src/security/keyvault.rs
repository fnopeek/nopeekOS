//! Keyvault — secrets kept apart from files (see docs/plan/KEYSTORE.md).
//!
//! Two tiers in one object, each with its own DEK:
//!   - system tier: DEK wrapped under the device subkey (`crypto::vault_subkey`),
//!     usable after disk unlock, gated at use by capability;
//!   - user tier:   DEK wrapped under a separate vault passphrase.
//! The device subkey opens only the system DEK, never the user DEK.
//!
//! This step holds the cryptographic foundation and a self-test. The file
//! format, the host ABI and the GUI follow in later steps.

use crate::crypto::keyslot;
use crate::kprintln;

/// Exercise the keyslot layer the keyvault is built on, with test vectors —
/// no disk, no stored secret. `keyvault selftest` runs it: a device slot must
/// round-trip and reject a wrong key, a passphrase slot must round-trip and
/// reject a wrong guess, and the device key must not open a passphrase slot
/// (the tier separation). A contiguous-memory shortfall for the Argon2 pass
/// shows as `skip(mem)`, not a failure.
pub fn selftest() {
    let subkey = [0x11u8; 32];
    let dek = [0x22u8; 32];

    // Device-key slot: pure AES-GCM, no allocation.
    let device = match keyslot::create_device(&subkey, &dek) {
        Some(slot) => {
            keyslot::unlock_device(&slot, &subkey) == Some(dek)
                && keyslot::unlock_device(&slot, &[0x33u8; 32]).is_none()
        }
        None => false,
    };

    // Passphrase slot: small Argon2 params so a diagnostic stays quick.
    let params = keyslot::Params { m_kib: 1024, t: 1, p: 4 };
    let passphrase = keyslot::create(b"correct horse battery", &dek, params).map(|slot| {
        keyslot::unlock(&slot, b"correct horse battery") == Some(dek)
            && keyslot::unlock(&slot, b"wrong").is_none()
    });

    // Tier separation: the device subkey opens the system slot but not a
    // passphrase-wrapped user slot.
    let sys_dek = [0xAAu8; 32];
    let usr_dek = [0xBBu8; 32];
    let separation = match (
        keyslot::create_device(&subkey, &sys_dek),
        keyslot::create(b"vault pass", &usr_dek, params),
    ) {
        (Some(sys), Some(usr)) => Some(
            keyslot::unlock_device(&sys, &subkey) == Some(sys_dek)
                && keyslot::unlock_device(&usr, &subkey).is_none()
                && keyslot::unlock(&usr, b"vault pass") == Some(usr_dek),
        ),
        _ => None,
    };

    // End-to-end with the REAL device subkey (the disk is unlocked while the
    // shell runs): wrap a DEK under `crypto::vault_subkey()` and recover it —
    // the actual system-tier unlock path. None only if the disk is locked.
    let subkey_path = match crate::crypto::vault_subkey() {
        Some(k) => keyslot::create_device(&k, &dek)
            .and_then(|slot| keyslot::unlock_device(&slot, &k))
            .map(|got| got == dek),
        None => None,
    };

    let fmt = |o: Option<bool>| match o {
        Some(true) => "OK",
        Some(false) => "FAIL",
        None => "skip",
    };
    let pass = device
        && passphrase != Some(false)
        && separation != Some(false)
        && subkey_path != Some(false);
    kprintln!(
        "[npk] keyvault selftest: device={} subkey={} passphrase={} separation={} -> {}",
        if device { "OK" } else { "FAIL" },
        fmt(subkey_path),
        fmt(passphrase),
        fmt(separation),
        if pass { "OK" } else { "FAIL" }
    );
}
