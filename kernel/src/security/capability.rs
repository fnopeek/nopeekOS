//! Capability System
//!
//! Security foundation of nopeekOS. No chmod, no ACLs, no users.
//! Every permission is a token with a random 256-bit ID (post-quantum safe).
//! Inspired by seL4 capabilities.

use bitflags::bitflags;
use spin::Mutex;
use crate::audit::{self, AuditOp, DenyReason};
use crate::interrupts;

/// 256-bit capability token ID (post-quantum: Grover-safe at 128-bit effective)
pub type CapId = [u8; 32];

/// Null capability (no access)
pub const CAP_NULL: CapId = [0u8; 32];

const MAX_CAPABILITIES: usize = 256;

static VAULT: Mutex<Vault> = Mutex::new(Vault::empty());
static ROOT_CAP: Mutex<CapId> = Mutex::new(CAP_NULL);

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rights: u32 {
        const READ      = 0b0000_0001;
        const WRITE     = 0b0000_0010;
        const EXECUTE   = 0b0000_0100;
        const DELEGATE  = 0b0000_1000;
        const REVOKE    = 0b0001_0000;
        const AUDIT     = 0b0010_0000;
        /// Phase 10: `npk_scene_commit` — widget-tree rendering.
        const RENDER    = 0b0100_0000;
        /// P10.10: `npk_canvas_commit` — upload a raw BGRA bitmap into a
        /// `Widget::Canvas` (image viewer, future paint app).
        const CANVAS    = 0b1000_0000;
        /// `npk_capture_screen` — read the composited framebuffer
        /// (screenshot tool). Highly privileged: screen-scrape.
        const CAPTURE   = 0b1_0000_0000;
        /// `npk_acpi_dsdt` / `npk_ec_read` / `npk_ec_write` — raw firmware +
        /// embedded-controller access (the AML battery driver). Highly
        /// privileged: direct hardware I/O.
        const HARDWARE  = 0b10_0000_0000;
        const ALL       = 0b11_1111_1111;
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
        self.expires_at.map_or(false, |exp| interrupts::ticks() >= exp)
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

    /// Create a new capability delegated from parent.
    /// Rights monotonicity: delegated rights ⊆ parent rights.
    /// Temporal monotonicity: child expiry ≤ parent expiry.
    pub fn create(
        &mut self,
        parent_id: CapId,
        resource: ResourceKind,
        rights: Rights,
        ttl_ticks: Option<u64>,
    ) -> Result<CapId, CapError> {
        let parent = self.find(&parent_id).ok_or(CapError::NotFound)?;
        if !parent.active {
            audit::record(AuditOp::Denied { reason: DenyReason::Revoked });
            return Err(CapError::Revoked);
        }
        if parent.is_expired() {
            audit::record(AuditOp::Denied { reason: DenyReason::Expired });
            return Err(CapError::Expired);
        }
        if !parent.rights.contains(Rights::DELEGATE) {
            audit::record(AuditOp::Denied { reason: DenyReason::InsufficientRights });
            return Err(CapError::InsufficientRights);
        }
        if !parent.rights.contains(rights) {
            audit::record(AuditOp::Denied { reason: DenyReason::EscalationAttempt });
            return Err(CapError::EscalationAttempt);
        }

        let slot = self.caps.iter().position(|c| c.is_none())
            .ok_or_else(|| {
                audit::record(AuditOp::Denied { reason: DenyReason::VaultFull });
                CapError::VaultFull
            })?;

        // Child expiry cannot exceed parent expiry
        let expires_at = match (ttl_ticks.map(|ttl| interrupts::ticks() + ttl), parent.expires_at) {
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

        audit::record(AuditOp::Create { parent_id, new_id: id });
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
        (&VAULT, root_id)
    }

    /// Revoke a capability and all its children (transitive)
    #[allow(dead_code)]
    pub fn revoke(&mut self, revoker_id: CapId, target_id: CapId) -> Result<(), CapError> {
        let revoker = self.find(&revoker_id).ok_or(CapError::NotFound)?;
        if !revoker.rights.contains(Rights::REVOKE) {
            audit::record(AuditOp::Denied { reason: DenyReason::InsufficientRights });
            return Err(CapError::InsufficientRights);
        }
        self.revoke_recursive(&target_id);
        audit::record(AuditOp::Revoke { revoker_id, target_id });
        Ok(())
    }

    /// Check if a capability grants the required rights
    pub fn check(&self, cap_id: &CapId, required: Rights) -> Result<&Capability, CapError> {
        let cap = self.find(cap_id).ok_or(CapError::NotFound)?;
        if !cap.active {
            audit::record(AuditOp::Denied { reason: DenyReason::Revoked });
            return Err(CapError::Revoked);
        }
        if cap.is_expired() {
            audit::record(AuditOp::Expired { cap_id: *cap_id });
            return Err(CapError::Expired);
        }
        if !cap.rights.contains(required) {
            audit::record(AuditOp::Denied { reason: DenyReason::InsufficientRights });
            return Err(CapError::InsufficientRights);
        }
        audit::record(AuditOp::Check { cap_id: *cap_id });
        Ok(cap)
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

    fn revoke_recursive(&mut self, target_id: &CapId) {
        for cap in self.caps.iter_mut().flatten() {
            if cap.id == *target_id { cap.active = false; }
        }
        let mut children = [[0u8; 32]; 64];
        let mut child_count = 0;
        for cap in self.caps.iter().filter_map(|c| c.as_ref()) {
            if cap.parent.as_ref() == Some(target_id) && cap.active && child_count < 64 {
                children[child_count] = cap.id;
                child_count += 1;
            }
        }
        for i in 0..child_count {
            self.revoke_recursive(&children[i]);
        }
    }
}

/// Create a capability for a WASM module (delegates from root internally).
pub fn create_module_cap(rights: Rights, ttl_ticks: Option<u64>) -> Result<CapId, CapError> {
    let root = *ROOT_CAP.lock();
    VAULT.lock().create(root, ResourceKind::Execute, rights, ttl_ticks)
}

// ── Per-app capability declaration (`.npk.caps` section) ──────────────
//
// A widget app self-declares the rights it needs in a 1-byte custom
// section. The spawn path (npk_spawn_module / launch_app / spawn_launcher)
// reads it and grants EXACTLY those rights — no blanket WRITE. The bit
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

fn rights_from_caps_byte(b: u8) -> Rights {
    let mut r = Rights::empty();
    if b & CAP_BIT_READ     != 0 { r |= Rights::READ; }
    if b & CAP_BIT_WRITE    != 0 { r |= Rights::WRITE; }
    if b & CAP_BIT_EXEC      != 0 { r |= Rights::EXECUTE; }
    if b & CAP_BIT_RENDER   != 0 { r |= Rights::RENDER; }
    if b & CAP_BIT_CAPTURE  != 0 { r |= Rights::CAPTURE; }
    if b & CAP_BIT_CANVAS   != 0 { r |= Rights::CANVAS; }
    if b & CAP_BIT_HARDWARE != 0 { r |= Rights::HARDWARE; }
    r
}

/// Rights granted to a widget app that ships no `.npk.caps` section:
/// read + execute + render, but NOT write. Matches the pre-per-app
/// behavior minus the blanket WRITE.
fn default_widget_rights() -> Rights {
    Rights::READ | Rights::EXECUTE | Rights::RENDER
}

/// Resolve the rights to grant a widget module from its `.npk.caps`
/// custom section, or the safe default if absent / malformed.
pub fn widget_rights_from_wasm(wasm: &[u8]) -> Rights {
    match extract_wasm_custom_section(wasm, ".npk.caps") {
        Some(s) if !s.is_empty() => rights_from_caps_byte(s[0]),
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

/// Create a capability for a WASM driver module bound to a specific PCI device.
pub fn create_driver_cap(
    bus: u8, device: u8, function: u8,
    rights: Rights, ttl_ticks: Option<u64>,
) -> Result<CapId, CapError> {
    let root = *ROOT_CAP.lock();
    VAULT.lock().create(root, ResourceKind::PciDevice { bus, device, function }, rights, ttl_ticks)
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
