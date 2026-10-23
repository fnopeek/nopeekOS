//! Keyvault — secrets kept apart from files (docs/plan/KEYSTORE.md).
//!
//! One object, two tiers, each with its own DEK:
//!   - system tier: DEK wrapped under the device subkey (`crypto::vault_subkey`)
//!     and, for migration, under the vault passphrase; used after disk unlock,
//!     gated at use by capability;
//!   - user tier:   DEK wrapped under the vault passphrase only.
//! The device subkey opens only the system DEK; the user DEK needs the
//! passphrase. Entries are AES-256-GCM under their tier's DEK with the name as
//! associated data, so an entry cannot be relabeled or read with the wrong DEK.
//!
//! This step holds the file format, the crypto engine and a self-test. The
//! host ABI (sys/vault, cap broker, demand-launch) and the GUI follow.

use alloc::string::String;
use alloc::vec::Vec;

use crate::crypto::{self, keyslot};
use crate::crypto::keyslot::{Slot, SLOT_BYTES};
use crate::kprintln;

pub const TIER_SYSTEM: u8 = 0;
pub const TIER_USER: u8 = 1;

const MAGIC: &[u8; 8] = b"NPKVAULT";
const VERSION: u16 = 1;
/// magic + version + reserved + three keyslots + entry count.
const HEADER_LEN: usize = 8 + 2 + 2 + SLOT_BYTES * 3 + 4;

struct Entry {
    tier: u8,
    name: String,
    nonce: [u8; 12],
    ct: Vec<u8>, // AES-256-GCM ciphertext + tag
}

/// A parsed, in-memory vault. The DEKs are never stored here — only the
/// keyslots that wrap them and the per-entry ciphertext.
pub struct VaultFile {
    sys_device: Slot, // system DEK under the device subkey
    sys_pass: Slot,   // system DEK under the vault passphrase (migration/recovery)
    usr_pass: Slot,   // user DEK under the vault passphrase
    entries: Vec<Entry>,
}

impl VaultFile {
    /// A fresh, empty vault: two random DEKs, each wrapped in its slots. The
    /// vault passphrase wraps both DEKs; the device subkey wraps only the
    /// system DEK.
    pub fn create(
        vault_pass: &[u8],
        device_subkey: &[u8; 32],
        params: keyslot::Params,
    ) -> Option<Self> {
        let sys_dek = crate::csprng::random_256();
        let usr_dek = crate::csprng::random_256();
        Some(VaultFile {
            sys_device: keyslot::create_device(device_subkey, &sys_dek)?,
            sys_pass: keyslot::create(vault_pass, &sys_dek, params)?,
            usr_pass: keyslot::create(vault_pass, &usr_dek, params)?,
            entries: Vec::new(),
        })
    }

    /// System DEK via the device subkey (the automatic, no-passphrase path).
    pub fn unlock_system_device(&self, device_subkey: &[u8; 32]) -> Option<[u8; 32]> {
        keyslot::unlock_device(&self.sys_device, device_subkey)
    }
    /// System DEK via the vault passphrase (migration onto a new device).
    pub fn unlock_system_pass(&self, pass: &[u8]) -> Option<[u8; 32]> {
        keyslot::unlock(&self.sys_pass, pass)
    }
    /// User DEK via the vault passphrase.
    pub fn unlock_user(&self, pass: &[u8]) -> Option<[u8; 32]> {
        keyslot::unlock(&self.usr_pass, pass)
    }

    fn aad(tier: u8, name: &str) -> Vec<u8> {
        let mut a = Vec::with_capacity(1 + name.len());
        a.push(tier);
        a.extend_from_slice(name.as_bytes());
        a
    }

    /// Insert or replace an entry in `tier`, encrypting `plaintext` under `dek`.
    pub fn set(&mut self, dek: &[u8; 32], tier: u8, name: &str, plaintext: &[u8]) -> bool {
        let rnd = crate::csprng::random_256();
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&rnd[..12]);
        let ct = crypto::aead_encrypt_aad(dek, &nonce, &Self::aad(tier, name), plaintext);
        if ct.is_empty() {
            return false;
        }
        let entry = Entry { tier, name: String::from(name), nonce, ct };
        match self.entries.iter_mut().find(|x| x.tier == tier && x.name == name) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
        true
    }

    /// The plaintext of an entry, decrypted with `dek`. None if absent or the
    /// DEK is wrong (the GCM tag fails) — so a system DEK cannot read a user
    /// entry, and vice versa.
    pub fn get(&self, dek: &[u8; 32], tier: u8, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.iter().find(|x| x.tier == tier && x.name == name)?;
        crypto::aead_decrypt_aad(dek, &e.nonce, &Self::aad(tier, name), &e.ct)
    }

    /// Entry names in a tier (no decryption needed).
    pub fn names(&self, tier: u8) -> Vec<&str> {
        self.entries
            .iter()
            .filter(|e| e.tier == tier)
            .map(|e| e.name.as_str())
            .collect()
    }

    /// Serialize to the on-object bytes.
    pub fn serialize(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.entries.len() * 64);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&self.sys_device);
        out.extend_from_slice(&self.sys_pass);
        out.extend_from_slice(&self.usr_pass);
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            out.push(e.tier);
            out.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            out.extend_from_slice(e.name.as_bytes());
            out.extend_from_slice(&e.nonce);
            out.extend_from_slice(&(e.ct.len() as u32).to_le_bytes());
            out.extend_from_slice(&e.ct);
        }
        out
    }

    /// Parse from bytes. Defensive: every length is checked against what
    /// remains, so a short or malformed blob returns None instead of indexing
    /// out of bounds. The bytes come from an AEAD-authenticated npkFS object,
    /// but are parsed as if they might not be.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER_LEN || &b[..8] != MAGIC {
            return None;
        }
        if u16::from_le_bytes([b[8], b[9]]) != VERSION {
            return None;
        }
        let mut off = 12;
        let mut sys_device = [0u8; SLOT_BYTES];
        sys_device.copy_from_slice(&b[off..off + SLOT_BYTES]);
        off += SLOT_BYTES;
        let mut sys_pass = [0u8; SLOT_BYTES];
        sys_pass.copy_from_slice(&b[off..off + SLOT_BYTES]);
        off += SLOT_BYTES;
        let mut usr_pass = [0u8; SLOT_BYTES];
        usr_pass.copy_from_slice(&b[off..off + SLOT_BYTES]);
        off += SLOT_BYTES;
        let count = u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]) as usize;
        off += 4;

        let mut entries = Vec::new();
        for _ in 0..count {
            if off + 3 > b.len() {
                return None;
            }
            let tier = b[off];
            let name_len = u16::from_le_bytes([b[off + 1], b[off + 2]]) as usize;
            off += 3;
            if off + name_len + 12 + 4 > b.len() {
                return None;
            }
            let name = String::from_utf8(b[off..off + name_len].to_vec()).ok()?;
            off += name_len;
            let mut nonce = [0u8; 12];
            nonce.copy_from_slice(&b[off..off + 12]);
            off += 12;
            let ct_len = u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]]) as usize;
            off += 4;
            if off + ct_len > b.len() {
                return None;
            }
            let ct = b[off..off + ct_len].to_vec();
            off += ct_len;
            entries.push(Entry { tier, name, nonce, ct });
        }
        Some(VaultFile { sys_device, sys_pass, usr_pass, entries })
    }
}

/// Exercise the keyslot layer and the vault file format with test vectors —
/// no disk object, no stored secret. `keyvault selftest` runs it. A
/// contiguous-memory shortfall for the Argon2 pass shows as `skip`, not a
/// failure.
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

    // Full vault file: create, add one entry per tier, serialize, re-parse,
    // unlock each tier, read back. Also check the two invariants that matter:
    // the system DEK cannot read a user entry, and the vault passphrase also
    // recovers the system DEK (the migration path). Uses the real device
    // subkey when the disk is unlocked.
    let device_subkey = crate::crypto::vault_subkey().unwrap_or(subkey);
    let file = VaultFile::create(b"vault pass", &device_subkey, params).map(|mut v| {
        let (Some(sys_dek), Some(usr_dek)) =
            (v.unlock_system_device(&device_subkey), v.unlock_user(b"vault pass"))
        else {
            return false;
        };
        v.set(&sys_dek, TIER_SYSTEM, "net:TestNet", b"pmk-material");
        v.set(&usr_dek, TIER_USER, "site:example", b"hunter2");
        let bytes = v.serialize();
        let Some(p) = VaultFile::parse(&bytes) else { return false };
        let (Some(sys2), Some(usr2)) =
            (p.unlock_system_device(&device_subkey), p.unlock_user(b"vault pass"))
        else {
            return false;
        };
        p.get(&sys2, TIER_SYSTEM, "net:TestNet").as_deref() == Some(b"pmk-material".as_slice())
            && p.get(&usr2, TIER_USER, "site:example").as_deref() == Some(b"hunter2".as_slice())
            && p.get(&sys2, TIER_USER, "site:example").is_none()      // system DEK can't read a user entry
            && p.unlock_system_pass(b"vault pass") == Some(sys2)      // migration path recovers the system DEK
            && p.names(TIER_SYSTEM).contains(&"net:TestNet")
            && p.names(TIER_USER).contains(&"site:example")
    });

    let fmt = |o: Option<bool>| match o {
        Some(true) => "OK",
        Some(false) => "FAIL",
        None => "skip",
    };
    let pass = device && passphrase != Some(false) && file != Some(false);
    kprintln!(
        "[npk] keyvault selftest: device={} passphrase={} file={} -> {}",
        if device { "OK" } else { "FAIL" },
        fmt(passphrase),
        fmt(file),
        if pass { "OK" } else { "FAIL" }
    );
}
