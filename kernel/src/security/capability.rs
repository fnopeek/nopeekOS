//! Capability System
//!
//! Security foundation of nopeekOS. No chmod, no ACLs, no users.
//! Every permission is a token with a random 256-bit ID (post-quantum safe).
//! Inspired by seL4 capabilities.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use bitflags::bitflags;
use spin::Mutex;
use crate::audit;

/// 256-bit capability token ID (post-quantum: Grover-safe at 128-bit effective)
pub type CapId = [u8; 32];

/// Null capability (no access)
pub const CAP_NULL: CapId = [0u8; 32];

const MAX_CAPABILITIES: usize = 256;

static VAULT: Mutex<Vault> = Mutex::new(Vault::empty());
static ROOT_CAP: Mutex<CapId> = Mutex::new(CAP_NULL);
/// Checks passed per vault slot since its capability was issued; the
/// journal gets the sum when the slot is released. Relaxed: a counter
/// read once at release, under the vault lock that also resets it.
static SLOT_CHECKS: [AtomicU64; MAX_CAPABILITIES] = [const { AtomicU64::new(0) }; MAX_CAPABILITIES];
/// Rights a check of the slot's capability asked for and got, as bits: what
/// the holder used of what it holds. Same lifetime and ordering as
/// `SLOT_CHECKS`.
static SLOT_USED: [AtomicU32; MAX_CAPABILITIES] = [const { AtomicU32::new(0) }; MAX_CAPABILITIES];

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rights: u32 {
        const READ      = 0b0000_0001;
        const WRITE     = 0b0000_0010;
        const EXECUTE   = 0b0000_0100;
        const DELEGATE  = 0b0000_1000;
        const REVOKE    = 0b0001_0000;
        const AUDIT     = 0b0010_0000;
        /// `npk_scene_commit` — widget-tree rendering.
        const RENDER    = 0b0100_0000;
        /// `npk_canvas_commit` — upload a raw BGRA bitmap into a
        /// `Widget::Canvas`.
        const CANVAS    = 0b1000_0000;
        /// `npk_capture_screen` — read the composited framebuffer
        /// (screenshot tool). Highly privileged: screen-scrape.
        const CAPTURE   = 0b1_0000_0000;
        /// `npk_acpi_dsdt` / `npk_ec_read` / `npk_ec_write` — raw firmware +
        /// embedded-controller access (the AML battery driver). Highly
        /// privileged: direct hardware I/O.
        const HARDWARE  = 0b10_0000_0000;
        /// `npk_wifi_send_cmd` / `npk_wifi_poll_event` — the WiFi-class control
        /// channel manager side (wifid.wasm supplicant). Privileged: can scan,
        /// connect, and read EAPOL handshake frames. See docs/spec/WIFI_CLASS_ABI.md.
        const NETCTL    = 0b100_0000_0000;
        /// `npk_http_request` — outbound HTTPS fetch for the native browser
        /// (beak). Distinct from NETCTL (WiFi-supplicant control): NET is
        /// simply "may make outbound TLS requests", the browser's fetch cap.
        const NET       = 0b1000_0000_0000;
        /// System UI roles: the panel, overlays that take the
        /// keyboard focus, modal windows, power off. An app that has them
        /// can put itself in front of the user and read what is typed, so
        /// only the shell's own modules declare it.
        const SHELL     = 0b1_0000_0000_0000;
        /// The `ext/` namespace: files on external drives. A module without
        /// it reaches such a file only through the open dialog.
        const EXTERNAL  = 0b10_0000_0000_0000;
        /// `npk_usb_*` on a video interface: the camera. Never a default;
        /// the kernel's status item is on while a stream is open.
        const CAMERA    = 0b100_0000_0000_0000;
        /// `npk_usb_*` on an audio interface, and audio capture.
        const MIC       = 0b1000_0000_0000_0000;
        /// `npk_setting_set`: change the system settings the settings
        /// table opens to apps (docs/spec/SETTINGS.md); the settings app.
        const SETTINGS  = 0b1_0000_0000_0000_0000;
        const ALL       = 0b1_1111_1111_1111_1111;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ResourceKind {
    Kernel,
    Memory { base: usize, size: usize },
    Serial,
    Network,
    Store,
    Display,
    Execute,
    PciDevice { bus: u8, device: u8, function: u8 },
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Capability {
    pub id: CapId,
    pub resource: ResourceKind,
    pub rights: Rights,
    pub parent: Option<CapId>,
    pub active: bool,
    /// Tick at which this capability expires. None = no expiry.
    pub expires_at: Option<u64>,
}

impl Capability {
    pub fn is_expired(&self) -> bool {
        self.expires_at.map_or(false, |exp| crate::time::ticks() >= exp)
    }
}

pub struct Vault {
    caps: [Option<Capability>; MAX_CAPABILITIES],
    count: usize,
}

impl Vault {
    const fn empty() -> Self {
        Vault { caps: [const { None }; MAX_CAPABILITIES], count: 0 }
    }

    /// Create a new capability delegated from parent, issued to `holder`
    /// (the module it is for; the journal names it).
    /// Rights monotonicity: delegated rights ⊆ parent rights.
    /// Temporal monotonicity: child expiry ≤ parent expiry.
    pub fn create(
        &mut self,
        parent_id: CapId,
        resource: ResourceKind,
        rights: Rights,
        ttl_ticks: Option<u64>,
        holder: &str,
    ) -> Result<CapId, CapError> {
        let refuse = |e: CapError| {
            let call = alloc::format!("issue {}", holder);
            audit::denied(&parent_id, rights, &call, e.reason());
            e
        };
        let parent = self.find(&parent_id).ok_or(CapError::NotFound)?;
        if !parent.active { return Err(refuse(CapError::Revoked)); }
        if parent.is_expired() { return Err(refuse(CapError::Expired)); }
        if !parent.rights.contains(Rights::DELEGATE) {
            return Err(refuse(CapError::InsufficientRights));
        }
        if !parent.rights.contains(rights) {
            return Err(refuse(CapError::EscalationAttempt));
        }

        // A free slot, or one whose capability is revoked or expired: those
        // only ever answer "denied", and a slot that is never reused turns
        // a few hundred launches into a full vault.
        let root = *ROOT_CAP.lock();
        let slot = self.caps.iter().position(|c| c.is_none())
            .or_else(|| self.caps.iter().position(|c| {
                c.as_ref().is_some_and(|c| c.id != root && (!c.active || c.is_expired()))
            }))
            .ok_or_else(|| refuse(CapError::VaultFull))?;
        if let Some(old) = &self.caps[slot] {
            audit::released(&old.id, SLOT_CHECKS[slot].load(Ordering::Relaxed), slot_used(slot));
        }

        // Child expiry cannot exceed parent expiry
        let expires_at = match (ttl_ticks.map(|ttl| crate::time::ticks() + ttl), parent.expires_at) {
            (Some(child), Some(parent_exp)) => Some(child.min(parent_exp)),
            (None, Some(parent_exp)) => Some(parent_exp),
            (exp, None) => exp,
        };

        let id = next_id();
        self.caps[slot] = Some(Capability {
            id, resource, rights,
            parent: Some(parent_id),
            active: true,
            expires_at,
        });
        self.count += 1;
        SLOT_CHECKS[slot].store(0, Ordering::Relaxed);
        SLOT_USED[slot].store(0, Ordering::Relaxed);

        audit::issued(&id, holder, rights);
        Ok(id)
    }

    /// Initialize vault with root capability. Returns vault ref + root cap ID.
    /// Requires csprng::init() to be called first.
    pub fn init() -> (&'static Mutex<Vault>, CapId) {
        let root_id = next_id();
        {
            let mut vault = VAULT.lock();
            vault.caps[0] = Some(Capability {
                id: root_id,
                resource: ResourceKind::Kernel,
                rights: Rights::ALL,
                parent: None,
                active: true,
                expires_at: None,
            });
            vault.count = 1;
        }
        *ROOT_CAP.lock() = root_id;
        audit::issued(&root_id, "system", Rights::ALL);
        (&VAULT, root_id)
    }

    /// Revoke a capability and all its children (transitive)
    #[allow(dead_code)]
    pub fn revoke(&mut self, revoker_id: CapId, target_id: CapId) -> Result<(), CapError> {
        let revoker = self.find(&revoker_id).ok_or(CapError::NotFound)?;
        if !revoker.rights.contains(Rights::REVOKE) {
            audit::denied(&revoker_id, Rights::REVOKE, "revoke", "");
            return Err(CapError::InsufficientRights);
        }
        self.revoke_recursive(&target_id);
        let target = alloc::format!("{:08x}", short_id(&target_id));
        audit::record(&revoker_id, audit::Kind::Revoke, &target);
        Ok(())
    }

    /// Check if a capability grants the required rights. Journals nothing:
    /// a failed check may be a probe; the gate that refuses journals it
    /// (`require`).
    pub fn check(&self, cap_id: &CapId, required: Rights) -> Result<&Capability, CapError> {
        let slot = self.caps.iter()
            .position(|c| c.as_ref().is_some_and(|c| c.id == *cap_id))
            .ok_or(CapError::NotFound)?;
        let Some(cap) = self.caps[slot].as_ref() else { return Err(CapError::NotFound) };
        if !cap.active { return Err(CapError::Revoked); }
        if cap.is_expired() { return Err(CapError::Expired); }
        if !cap.rights.contains(required) { return Err(CapError::InsufficientRights); }
        audit::record_check_passed();
        SLOT_CHECKS[slot].fetch_add(1, Ordering::Relaxed);
        SLOT_USED[slot].fetch_or(required.bits(), Ordering::Relaxed);
        Ok(cap)
    }

    /// What `cap` holds, what it used of it and how many checks it passed
    /// since it was issued; `None` if it is not here.
    pub fn usage_of(&self, cap_id: &CapId) -> Option<(Rights, Rights, u64)> {
        let i = self.caps.iter().position(|c| c.as_ref().is_some_and(|c| c.id == *cap_id))?;
        let held = self.caps[i].as_ref()?.rights;
        Some((held, slot_used(i), SLOT_CHECKS[i].load(Ordering::Relaxed)))
    }

    pub fn stats(&self) -> (usize, usize) {
        let active = self.caps.iter()
            .filter(|c| c.as_ref().map_or(false, |c| c.active && !c.is_expired()))
            .count();
        (active, MAX_CAPABILITIES)
    }

    fn find(&self, id: &CapId) -> Option<&Capability> {
        self.caps.iter().filter_map(|c| c.as_ref()).find(|c| c.id == *id)
    }

    /// Deactivate `target_id` and everything delegated from it, however
    /// many: repeat until a pass finds no active child of a revoked parent.
    fn revoke_recursive(&mut self, target_id: &CapId) {
        for cap in self.caps.iter_mut().flatten() {
            if cap.id == *target_id { cap.active = false; }
        }
        loop {
            let revoked: alloc::vec::Vec<CapId> = self.caps.iter().flatten()
                .filter(|c| !c.active).map(|c| c.id).collect();
            let mut changed = false;
            for cap in self.caps.iter_mut().flatten() {
                if cap.active && cap.parent.is_some_and(|p| revoked.contains(&p)) {
                    cap.active = false;
                    changed = true;
                }
            }
            if !changed { break; }
        }
    }

    /// Revoke `id` and its delegations and free their slots.
    fn release(&mut self, id: &CapId) {
        self.revoke_recursive(id);
        for (i, slot) in self.caps.iter_mut().enumerate() {
            if let Some(gone) = slot.as_ref().filter(|c| !c.active).map(|c| c.id) {
                audit::released(&gone, SLOT_CHECKS[i].load(Ordering::Relaxed), slot_used(i));
                *slot = None;
                self.count = self.count.saturating_sub(1);
            }
        }
    }
}

/// Create a capability for the WASM module `name` (delegates from root).
pub fn create_module_cap(name: &str, rights: Rights, ttl_ticks: Option<u64>) -> Result<CapId, CapError> {
    let root = *ROOT_CAP.lock();
    VAULT.lock().create(root, ResourceKind::Execute, rights, ttl_ticks, name)
}

/// A module instance has ended: its capability (one per launch) and what it
/// delegated are revoked and their vault slots freed. Never the root.
pub fn release_module_cap(cap: &CapId) {
    if *cap == *ROOT_CAP.lock() { return; }
    VAULT.lock().release(cap);
}


// ── Per-path grants ───────────────────────────────────────────────────
//
// The file dialog's whole point: a user picking a file in trusted UI is
// the authorisation, so the app that asked never needs blanket WRITE.
// `npk_pick` records a grant for exactly the chosen path against the
// requester's capability; `npk_store` accepts it in place of the global
// right.
//
// Narrow on purpose:
//   - one exact path, or, for a folder the user opened in the dialog's
//     folder mode, that folder and everything below it; a folder grant
//     matches at path boundaries only and never a path with an empty,
//     `.` or `..` segment
//   - no folder grant on the root, `sys`, `.system`, `priv` or `ext`
//     (`grant_tree` refuses)
//   - bound to the capability of the instance that asked, so it dies
//     with the app
//   - never covers `sys/wasm/` — that check runs before this one and a
//     grant must not be a way around it
//
// Bounded per capability, not per system: one app's grants never push out
// another's. Past the bound that app's oldest grant goes, which at worst
// costs its user one re-pick; all of them go when the instance ends.

const MAX_PATH_GRANTS: usize = 64;

#[derive(Clone)]
struct PathGrant {
    cap: CapId,
    path: alloc::string::String,
    rights: Rights,
    /// `path` is a folder and the grant covers everything below it.
    tree: bool,
}

static PATH_GRANTS: Mutex<alloc::vec::Vec<PathGrant>> = Mutex::new(alloc::vec::Vec::new());

/// Record that `cap` may act on `path` with `rights`. Re-granting the
/// same pair widens the existing entry rather than stacking duplicates.
pub fn grant_path(cap: CapId, path: &str, rights: Rights) {
    push_grant(cap, path, rights, false);
    audit::granted(&cap, path, rights);
}

/// Record that `cap` may act on folder `dir` and everything below it.
/// Refuses (false) the root, `sys`, `priv`, `.system` and anything below
/// those, and a path that is not in canonical form.
pub fn grant_tree(cap: CapId, dir: &str, rights: Rights) -> bool {
    if !canonical(dir) { return false; }
    let top = dir.split('/').next().unwrap_or("");
    if top == "sys" || top == ".system" || top == crate::wasm::PRIVATE_ROOT { return false; }
    if dir == crate::ext::ROOT { return false; }
    push_grant(cap, dir, rights, true);
    audit::granted(&cap, &alloc::format!("{}/", dir), rights);
    true
}

/// A non-empty relative path of plain names: no leading or trailing `/`,
/// no empty, `.` or `..` segment. Only such paths are matched against a
/// folder grant, so no spelling of a path can reach outside the folder.
fn canonical(path: &str) -> bool {
    !path.is_empty() && path.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

fn push_grant(cap: CapId, path: &str, rights: Rights, tree: bool) {
    let mut g = PATH_GRANTS.lock();
    if let Some(e) = g.iter_mut().find(|e| e.cap == cap && e.path == path && e.tree == tree) {
        e.rights |= rights;
        return;
    }
    if g.iter().filter(|e| e.cap == cap).count() >= MAX_PATH_GRANTS {
        if let Some(i) = g.iter().position(|e| e.cap == cap) { g.remove(i); }
    }
    g.push(PathGrant { cap, path: alloc::string::String::from(path), rights, tree });
}

/// True if `cap` holds a grant covering `path` with `rights`: one for
/// exactly that path, or a folder grant for a folder it lies below.
pub fn check_path_grant(cap: &CapId, path: &str, rights: Rights) -> bool {
    let canon = canonical(path);
    PATH_GRANTS.lock().iter().any(|e| {
        &e.cap == cap && e.rights.contains(rights) && (e.path == path
            || (e.tree && canon && path.len() > e.path.len()
                && path.starts_with(e.path.as_str())
                && path.as_bytes()[e.path.len()] == b'/'))
    })
}

/// Drop every grant held by `cap` — called when an instance ends, so a
/// recycled capability id can never inherit someone else's file.
pub fn revoke_path_grants(cap: &CapId) {
    PATH_GRANTS.lock().retain(|e| &e.cap != cap);
}

// ── Per-app capability declaration (`.npk.caps` section) ──────────────
//
// A widget app self-declares the rights it needs in a 1-byte custom
// section. The spawn path (npk_spawn_module / launch_app / spawn_launcher)
// reads it and grants exactly those rights — no blanket WRITE. The bit
// layout mirrors `nopeek_widgets::caps` in the SDK. An absent or
// malformed section falls back to a safe default that never includes
// WRITE, so a future app cannot silently gain write access.

const CAP_BIT_READ:    u8 = 0x01;
const CAP_BIT_WRITE:   u8 = 0x02;
const CAP_BIT_EXEC:    u8 = 0x04;
const CAP_BIT_RENDER:  u8 = 0x08;
const CAP_BIT_CAPTURE: u8 = 0x10;
const CAP_BIT_CANVAS:  u8 = 0x20;
const CAP_BIT_HARDWARE: u8 = 0x40;
const CAP_BIT_NETCTL:   u8 = 0x80;

fn rights_from_caps_byte(b: u8) -> Rights {
    let mut r = Rights::empty();
    if b & CAP_BIT_READ     != 0 { r |= Rights::READ; }
    if b & CAP_BIT_WRITE    != 0 { r |= Rights::WRITE; }
    if b & CAP_BIT_EXEC      != 0 { r |= Rights::EXECUTE; }
    if b & CAP_BIT_RENDER   != 0 { r |= Rights::RENDER; }
    if b & CAP_BIT_CAPTURE  != 0 { r |= Rights::CAPTURE; }
    if b & CAP_BIT_CANVAS   != 0 { r |= Rights::CANVAS; }
    if b & CAP_BIT_HARDWARE != 0 { r |= Rights::HARDWARE; }
    if b & CAP_BIT_NETCTL   != 0 { r |= Rights::NETCTL; }
    r
}

// Second `.npk.caps` byte — the first byte's 8 bits are full. Apps that
// need an extension right ship a 2-byte section; a 1-byte (or absent)
// section grants nothing from byte 2.
const CAP2_BIT_NET: u8 = 0x01; // npk_http_request (beak's outbound fetch)
const CAP2_BIT_SHELL: u8 = 0x02; // system UI roles (panel, launcher, picker)
const CAP2_BIT_EXTERNAL: u8 = 0x04; // files on external drives (`ext/`)
const CAP2_BIT_CAMERA: u8 = 0x08; // a camera's USB interfaces
const CAP2_BIT_MIC: u8 = 0x10; // a microphone
const CAP2_BIT_SETTINGS: u8 = 0x20; // writing system settings
const CAP2_BIT_AUDIT: u8 = 0x40; // reading the audit journal (peek)

fn rights_from_caps_byte2(b: u8) -> Rights {
    let mut r = Rights::empty();
    if b & CAP2_BIT_NET != 0 { r |= Rights::NET; }
    if b & CAP2_BIT_SHELL != 0 { r |= Rights::SHELL; }
    if b & CAP2_BIT_EXTERNAL != 0 { r |= Rights::EXTERNAL; }
    if b & CAP2_BIT_CAMERA != 0 { r |= Rights::CAMERA; }
    if b & CAP2_BIT_MIC != 0 { r |= Rights::MIC; }
    if b & CAP2_BIT_SETTINGS != 0 { r |= Rights::SETTINGS; }
    if b & CAP2_BIT_AUDIT != 0 { r |= Rights::AUDIT; }
    r
}

/// Rights granted to a widget app that ships no `.npk.caps` section:
/// read + execute + render, but not write.
fn default_widget_rights() -> Rights {
    Rights::READ | Rights::EXECUTE | Rights::RENDER
}

/// Resolve the rights to grant a widget module from its `.npk.caps`
/// custom section, or the safe default if absent / malformed.
pub fn widget_rights_from_wasm(wasm: &[u8]) -> Rights {
    match extract_wasm_custom_section(wasm, ".npk.caps") {
        Some(s) if !s.is_empty() => {
            let mut r = rights_from_caps_byte(s[0]);
            if s.len() >= 2 { r |= rights_from_caps_byte2(s[1]); }
            r
        }
        _ => default_widget_rights(),
    }
}

/// Minimal wasm custom-section reader (mirrors the SDK's app_catalog
/// parser): walks the module's sections and returns the payload of the
/// first custom section (id 0) whose name matches `target`.
fn extract_wasm_custom_section<'a>(wasm: &'a [u8], target: &str) -> Option<&'a [u8]> {
    if wasm.len() < 8 || &wasm[0..4] != b"\0asm" || &wasm[4..8] != [0x01, 0x00, 0x00, 0x00] {
        return None;
    }
    let mut cur = &wasm[8..];
    while !cur.is_empty() {
        let section_id = cur[0];
        cur = &cur[1..];
        let (size, consumed) = read_leb128_u32(cur)?;
        cur = &cur[consumed..];
        if size as usize > cur.len() { return None; }
        let (payload, rest) = cur.split_at(size as usize);
        cur = rest;
        if section_id != 0 { continue; }
        let (name_len, consumed) = read_leb128_u32(payload)?;
        let name_end = consumed + name_len as usize;
        if name_end > payload.len() { continue; }
        if &payload[consumed..name_end] == target.as_bytes() {
            return Some(&payload[name_end..]);
        }
    }
    None
}

fn read_leb128_u32(buf: &[u8]) -> Option<(u32, usize)> {
    let mut result: u32 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in buf.iter().enumerate() {
        if shift >= 32 { return None; }
        result |= ((b & 0x7F) as u32) << shift;
        if b & 0x80 == 0 { return Some((result, i + 1)); }
        shift += 7;
    }
    None
}

/// Check a capability against the global vault.
pub fn check_global(cap_id: &CapId, required: Rights) -> Result<(), CapError> {
    VAULT.lock().check(cap_id, required).map(|_| ())
}

/// The gate of `call`: whether `cap_id` holds `required`. A no is a
/// refusal and goes into the journal with the call.
pub fn require(cap_id: &CapId, required: Rights, call: &str) -> bool {
    match check_global(cap_id, required) {
        Ok(()) => true,
        Err(e) => {
            audit::denied(cap_id, required, call, e.reason());
            false
        }
    }
}

/// Whether `cap_id` holds `required`, asked to choose a path, not to
/// refuse; a no is not journalled.
pub fn holds(cap_id: &CapId, required: Rights) -> bool {
    check_global(cap_id, required).is_ok()
}

/// `READ|WRITE|NET`, `ALL`, or `-` for none.
pub fn rights_names(r: Rights) -> alloc::string::String {
    use alloc::string::String;
    if r.is_empty() { return String::from("-"); }
    if r == Rights::ALL { return String::from("ALL"); }
    let mut out = String::new();
    for (name, _) in r.iter_names() {
        if !out.is_empty() { out.push('|'); }
        out.push_str(name);
    }
    out
}

/// What `cap_id` holds, what it used of it and its passed checks.
pub fn usage_of(cap_id: &CapId) -> Option<(Rights, Rights, u64)> {
    VAULT.lock().usage_of(cap_id)
}

fn slot_used(slot: usize) -> Rights {
    Rights::from_bits_truncate(SLOT_USED[slot].load(Ordering::Relaxed))
}

/// Create a capability for the driver module `name`, bound to one PCI device.
pub fn create_driver_cap(
    name: &str, bus: u8, device: u8, function: u8,
    rights: Rights, ttl_ticks: Option<u64>,
) -> Result<CapId, CapError> {
    let root = *ROOT_CAP.lock();
    VAULT.lock().create(root, ResourceKind::PciDevice { bus, device, function }, rights, ttl_ticks, name)
}

/// Check that a capability grants access to a specific PCI device.
pub fn check_pci_device(
    cap_id: &CapId, required: Rights, bus: u8, device: u8, function: u8,
) -> Result<(), CapError> {
    let vault = VAULT.lock();
    let cap = vault.check(cap_id, required)?;
    match cap.resource {
        ResourceKind::PciDevice { bus: b, device: d, function: f }
            if b == bus && d == device && f == function => Ok(()),
        ResourceKind::Kernel => Ok(()),
        _ => Err(CapError::InsufficientRights),
    }
}

/// Short hex representation of a 256-bit cap ID (first 8 hex chars = 4 bytes)
pub fn short_id(id: &CapId) -> u32 {
    u32::from_be_bytes([id[0], id[1], id[2], id[3]])
}

#[derive(Debug)]
pub enum CapError {
    NotFound,
    Revoked,
    Expired,
    InsufficientRights,
    EscalationAttempt,
    VaultFull,
}

impl CapError {
    /// The journal's word for a refusal; empty for missing rights, the
    /// common case the rights column already says.
    pub fn reason(&self) -> &'static str {
        match self {
            CapError::NotFound => "no token",
            CapError::Revoked => "revoked",
            CapError::Expired => "expired",
            CapError::InsufficientRights => "",
            CapError::EscalationAttempt => "escalation",
            CapError::VaultFull => "vault full",
        }
    }
}

impl core::fmt::Display for CapError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            CapError::NotFound => write!(f, "capability not found"),
            CapError::Revoked => write!(f, "capability has been revoked"),
            CapError::Expired => write!(f, "capability has expired"),
            CapError::InsufficientRights => write!(f, "insufficient rights"),
            CapError::EscalationAttempt => write!(f, "privilege escalation denied"),
            CapError::VaultFull => write!(f, "capability vault full"),
        }
    }
}

fn next_id() -> CapId {
    crate::csprng::random_256()
}
