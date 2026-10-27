//! Keyvault — secrets kept apart from files.
//!
//! One object (`sys/vault`), two tiers, each with its own DEK:
//!   - system tier: DEK wrapped under the device subkey (`crypto::vault_subkey`)
//!     and, once a passphrase is set, also under it (migration); used after disk
//!     unlock, gated at use by capability;
//!   - user tier:   DEK wrapped under the vault passphrase only — exists only
//!     after the user sets a passphrase.
//! The device subkey opens only the system DEK; the user DEK needs the
//! passphrase. Entries are AES-256-GCM under their tier's DEK with the name as
//! associated data, so an entry cannot be relabeled or read with the wrong DEK.
//!
//! The kernel is the authority: it holds this engine, reads/writes the object
//! through npkFS, and holds the unlocked user DEK in a session. Modules and
//! wifid reach secrets only through this layer (the host ABI follows). The
//! GUI is a separate step.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use spin::Mutex;

use crate::crypto::keyslot::{self, Slot, SLOT_BYTES};
use crate::crypto::{self};
use crate::kprintln;

pub const TIER_SYSTEM: u8 = 0;
pub const TIER_USER: u8 = 1;

const MAGIC: &[u8; 8] = b"NPKVAULT";
const VERSION: u16 = 1;
/// magic + version + reserved + three keyslots + entry count.
const HEADER_LEN: usize = 8 + 2 + 2 + SLOT_BYTES * 3 + 4;

/// The vault object's path. Trust-critical (`wasm::is_trust_critical_path`), so
/// no app can touch the raw store through the generic FS path.
const VAULT_OBJECT: &str = "sys/vault";

/// The unlocked user DEK while the user tier is open. None when locked.
static USER_SESSION: Mutex<Option<[u8; 32]>> = Mutex::new(None);

struct Entry {
    tier: u8,
    name: String,
    nonce: [u8; 12],
    ct: Vec<u8>, // AES-256-GCM ciphertext + tag
}

/// A parsed, in-memory vault. The DEKs are never stored here — only the
/// keyslots that wrap them and the per-entry ciphertext. `sys_pass`/`usr_pass`
/// are empty (`keyslot::is_empty`) until a passphrase is set.
pub struct VaultFile {
    sys_device: Slot,
    sys_pass: Slot,
    usr_pass: Slot,
    entries: Vec<Entry>,
}

impl VaultFile {
    /// A fresh vault with only the system tier, wrapped under the device
    /// subkey — no passphrase yet, so WLAN and other system credentials work
    /// right after disk unlock. The user tier is added by `set_passphrase`.
    pub fn create(device_subkey: &[u8; 32]) -> Option<Self> {
        let sys_dek = crate::csprng::random_256();
        Some(VaultFile {
            sys_device: keyslot::create_device(device_subkey, &sys_dek)?,
            sys_pass: [0u8; SLOT_BYTES],
            usr_pass: [0u8; SLOT_BYTES],
            entries: Vec::new(),
        })
    }

    pub fn has_user_tier(&self) -> bool {
        !keyslot::is_empty(&self.usr_pass)
    }

    /// Set up the user tier (first passphrase), or change the passphrase. On
    /// first setup `old` is ignored and a new user DEK is drawn; on a change
    /// `old` must open the user tier, and both DEKs are re-wrapped under `new`.
    pub fn set_passphrase(
        &mut self,
        device_subkey: &[u8; 32],
        old: Option<&[u8]>,
        new: &[u8],
        params: keyslot::Params,
    ) -> bool {
        let Some(sys_dek) = self.unlock_system_device(device_subkey) else {
            return false;
        };
        let user_dek = if self.has_user_tier() {
            let Some(old) = old else { return false };
            let Some(d) = self.unlock_user(old) else { return false };
            d
        } else {
            crate::csprng::random_256()
        };
        let (Some(sp), Some(up)) = (
            keyslot::create(new, &sys_dek, params),
            keyslot::create(new, &user_dek, params),
        ) else {
            return false;
        };
        self.sys_pass = sp;
        self.usr_pass = up;
        true
    }

    pub fn unlock_system_device(&self, device_subkey: &[u8; 32]) -> Option<[u8; 32]> {
        keyslot::unlock_device(&self.sys_device, device_subkey)
    }
    pub fn unlock_system_pass(&self, pass: &[u8]) -> Option<[u8; 32]> {
        keyslot::unlock(&self.sys_pass, pass)
    }
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
        let entry = Entry { tier, name: name.to_string(), nonce, ct };
        match self.entries.iter_mut().find(|x| x.tier == tier && x.name == name) {
            Some(slot) => *slot = entry,
            None => self.entries.push(entry),
        }
        true
    }

    /// Remove an entry from `tier`, if present.
    pub fn delete(&mut self, tier: u8, name: &str) {
        self.entries.retain(|e| !(e.tier == tier && e.name == name));
    }

    /// The plaintext of an entry, decrypted with `dek`. None if absent or the
    /// DEK is wrong (the GCM tag fails) — so a system DEK cannot read a user
    /// entry, and vice versa.
    pub fn get(&self, dek: &[u8; 32], tier: u8, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.iter().find(|x| x.tier == tier && x.name == name)?;
        crypto::aead_decrypt_aad(dek, &e.nonce, &Self::aad(tier, name), &e.ct)
    }

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

// ── Persistence + session (the kernel-side keyvault authority) ─────────────

fn load() -> Option<VaultFile> {
    match crate::npkfs::fetch(VAULT_OBJECT) {
        Ok((bytes, _)) => VaultFile::parse(&bytes),
        Err(_) => None,
    }
}

fn save(v: &VaultFile) -> bool {
    crate::npkfs::upsert(VAULT_OBJECT, &v.serialize(), crate::security::capability::CAP_NULL).is_ok()
}

/// Load the vault, creating a device-only one if it does not exist yet. None
/// only while the disk is locked (no device subkey).
fn ensure() -> Option<VaultFile> {
    if let Some(v) = load() {
        return Some(v);
    }
    let subkey = crate::crypto::vault_subkey()?;
    let v = VaultFile::create(&subkey)?;
    save(&v).then_some(v)
}

/// Argon2 cost for the vault passphrase — the same calibration the disk uses.
fn params() -> keyslot::Params {
    keyslot::calibrate()
}

pub fn exists() -> bool {
    crate::npkfs::exists(VAULT_OBJECT)
}

/// Whether a user tier (a passphrase) has been set up.
pub fn user_tier_present() -> bool {
    load().map(|v| v.has_user_tier()).unwrap_or(false)
}

/// Create the vault (device-only system tier) if it does not exist.
pub fn init() -> bool {
    ensure().is_some()
}

/// Set or change the vault passphrase (adds/updates the user tier and the
/// system migration slot).
pub fn set_passphrase(old: Option<&[u8]>, new: &[u8]) -> bool {
    let Some(subkey) = crate::crypto::vault_subkey() else { return false };
    let Some(mut v) = ensure() else { return false };
    if !v.set_passphrase(&subkey, old, new, params()) {
        return false;
    }
    if !save(&v) {
        return false;
    }
    // Leave the user tier open: the caller just proved the passphrase, so a
    // fresh setup lands in an unlocked vault (otherwise the first write after
    // setup fails with no session, and nothing is stored).
    if let Some(dek) = v.unlock_user(new) {
        *USER_SESSION.lock() = Some(dek);
    }
    true
}

/// Open the user tier for this session.
pub fn unlock_user(pass: &[u8]) -> bool {
    let Some(v) = load() else { return false };
    match v.unlock_user(pass) {
        Some(dek) => {
            *USER_SESSION.lock() = Some(dek);
            true
        }
        None => false,
    }
}

/// Close the user tier: the DEK leaves memory.
pub fn lock_user() {
    let mut s = USER_SESSION.lock();
    if let Some(dek) = s.as_mut() {
        dek.fill(0);
    }
    *s = None;
}

pub fn user_unlocked() -> bool {
    USER_SESSION.lock().is_some()
}

/// Read a user secret. Requires the user tier to be unlocked this session.
pub fn user_get(name: &str) -> Option<Vec<u8>> {
    let dek = (*USER_SESSION.lock())?;
    load()?.get(&dek, TIER_USER, name)
}

/// Write a user secret. Requires the user tier to be unlocked this session.
pub fn user_set(name: &str, value: &[u8]) -> bool {
    let Some(dek) = *USER_SESSION.lock() else { return false };
    let Some(mut v) = load() else { return false };
    if !v.set(&dek, TIER_USER, name, value) {
        return false;
    }
    save(&v)
}

pub fn user_list() -> Vec<String> {
    match (*USER_SESSION.lock(), load()) {
        (Some(_), Some(v)) => v.names(TIER_USER).iter().map(|s| s.to_string()).collect(),
        _ => Vec::new(),
    }
}

/// Remove a user secret. Requires the user tier unlocked this session.
pub fn user_delete(name: &str) -> bool {
    if USER_SESSION.lock().is_none() {
        return false;
    }
    let Some(mut v) = load() else { return false };
    v.delete(TIER_USER, name);
    save(&v)
}

/// Read a system credential. The device subkey opens the system tier; the
/// CALLER must have checked the capability for this credential class. None
/// while the disk is locked or the entry is absent.
pub fn system_get(name: &str) -> Option<Vec<u8>> {
    let subkey = crate::crypto::vault_subkey()?;
    let v = load()?;
    let dek = v.unlock_system_device(&subkey)?;
    v.get(&dek, TIER_SYSTEM, name)
}

/// Write a system credential (creates the vault device-only if needed).
pub fn system_set(name: &str, value: &[u8]) -> bool {
    let Some(subkey) = crate::crypto::vault_subkey() else { return false };
    let Some(mut v) = ensure() else { return false };
    let Some(dek) = v.unlock_system_device(&subkey) else { return false };
    if !v.set(&dek, TIER_SYSTEM, name, value) {
        return false;
    }
    save(&v)
}

pub fn system_list() -> Vec<String> {
    match load() {
        Some(v) => v.names(TIER_SYSTEM).iter().map(|s| s.to_string()).collect(),
        None => Vec::new(),
    }
}

/// Remove a system credential.
pub fn system_delete(name: &str) -> bool {
    let Some(mut v) = load() else { return false };
    v.delete(TIER_SYSTEM, name);
    save(&v)
}

// ── Self-test ──────────────────────────────────────────────────────────────

/// Exercise the keyslot layer and the vault file format with test vectors —
/// no disk object, no stored secret. `keyvault selftest` runs it. A
/// contiguous-memory shortfall for the Argon2 pass shows as `skip`.
pub fn selftest() {
    let subkey = [0x11u8; 32];
    let dek = [0x22u8; 32];

    let device = match keyslot::create_device(&subkey, &dek) {
        Some(slot) => {
            keyslot::unlock_device(&slot, &subkey) == Some(dek)
                && keyslot::unlock_device(&slot, &[0x33u8; 32]).is_none()
        }
        None => false,
    };

    let p = keyslot::Params { m_kib: 1024, t: 1, p: 4 };
    let passphrase = keyslot::create(b"correct horse battery", &dek, p).map(|slot| {
        keyslot::unlock(&slot, b"correct horse battery") == Some(dek)
            && keyslot::unlock(&slot, b"wrong").is_none()
    });

    // Full vault file: create device-only, add a system entry (no passphrase
    // needed), set a passphrase, add a user entry, serialize, re-parse, unlock
    // each tier, read back. Invariants: the system DEK cannot read a user
    // entry, and the passphrase recovers the system DEK (the migration path).
    // Uses the real device subkey when the disk is unlocked.
    let device_subkey = crate::crypto::vault_subkey().unwrap_or(subkey);
    let file = VaultFile::create(&device_subkey).map(|mut v| {
        let Some(sys_dek) = v.unlock_system_device(&device_subkey) else { return false };
        v.set(&sys_dek, TIER_SYSTEM, "net:TestNet", b"pmk-material");
        if !v.set_passphrase(&device_subkey, None, b"vault pass", p) {
            return false;
        }
        let Some(usr_dek) = v.unlock_user(b"vault pass") else { return false };
        v.set(&usr_dek, TIER_USER, "site:example", b"hunter2");

        let Some(q) = VaultFile::parse(&v.serialize()) else { return false };
        let (Some(sys2), Some(usr2)) =
            (q.unlock_system_device(&device_subkey), q.unlock_user(b"vault pass"))
        else {
            return false;
        };
        q.get(&sys2, TIER_SYSTEM, "net:TestNet").as_deref() == Some(b"pmk-material".as_slice())
            && q.get(&usr2, TIER_USER, "site:example").as_deref() == Some(b"hunter2".as_slice())
            && q.get(&sys2, TIER_USER, "site:example").is_none()
            && q.unlock_system_pass(b"vault pass") == Some(sys2)
            && q.names(TIER_SYSTEM).contains(&"net:TestNet")
            && q.names(TIER_USER).contains(&"site:example")
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
