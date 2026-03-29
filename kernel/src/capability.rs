//! Capability System
//!
//! Security foundation of nopeekOS. No chmod, no ACLs, no users.
//! Every permission is a token with a random 128-bit ID.
//! Inspired by seL4 capabilities.

use bitflags::bitflags;
use spin::Mutex;
use crate::audit::{self, AuditOp, DenyReason};
use crate::interrupts;

const MAX_CAPABILITIES: usize = 256;

static VAULT: Mutex<Vault> = Mutex::new(Vault::empty());
static PRNG: Mutex<Xorshift128> = Mutex::new(Xorshift128::new());
static ROOT_CAP: Mutex<u128> = Mutex::new(0);

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Rights: u32 {
        const READ      = 0b0000_0001;
        const WRITE     = 0b0000_0010;
        const EXECUTE   = 0b0000_0100;
        const DELEGATE  = 0b0000_1000;
        const REVOKE    = 0b0001_0000;
        const AUDIT     = 0b0010_0000;
        const ALL       = 0b0011_1111;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Kernel,
    Memory { base: usize, size: usize },
    Serial,
    Network,
    Store,
    Display,
    Execute,
}

#[derive(Debug, Clone)]
pub struct Capability {
    pub id: u128,
    pub resource: ResourceKind,
    pub rights: Rights,
    pub parent: Option<u128>,
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
        parent_id: u128,
        resource: ResourceKind,
        rights: Rights,
        ttl_ticks: Option<u64>,
    ) -> Result<u128, CapError> {
        let parent = self.find(parent_id).ok_or(CapError::NotFound)?;
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
    /// Also stores root cap for internal use (module cap creation).
    pub fn init() -> (&'static Mutex<Vault>, u128) {
        seed_prng();
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
    pub fn revoke(&mut self, revoker_id: u128, target_id: u128) -> Result<(), CapError> {
        let revoker = self.find(revoker_id).ok_or(CapError::NotFound)?;
        if !revoker.rights.contains(Rights::REVOKE) {
            audit::record(AuditOp::Denied { reason: DenyReason::InsufficientRights });
            return Err(CapError::InsufficientRights);
        }
        self.revoke_recursive(target_id);
        audit::record(AuditOp::Revoke { revoker_id, target_id });
        Ok(())
    }

    /// Check if a capability grants the required rights
    pub fn check(&self, cap_id: u128, required: Rights) -> Result<&Capability, CapError> {
        let cap = self.find(cap_id).ok_or(CapError::NotFound)?;
        if !cap.active {
            audit::record(AuditOp::Denied { reason: DenyReason::Revoked });
            return Err(CapError::Revoked);
        }
        if cap.is_expired() {
            audit::record(AuditOp::Expired { cap_id });
            return Err(CapError::Expired);
        }
        if !cap.rights.contains(required) {
            audit::record(AuditOp::Denied { reason: DenyReason::InsufficientRights });
            return Err(CapError::InsufficientRights);
        }
        audit::record(AuditOp::Check { cap_id });
        Ok(cap)
    }

    pub fn stats(&self) -> (usize, usize) {
        let active = self.caps.iter()
            .filter(|c| c.as_ref().map_or(false, |c| c.active && !c.is_expired()))
            .count();
        (active, MAX_CAPABILITIES)
    }

    fn find(&self, id: u128) -> Option<&Capability> {
        self.caps.iter().filter_map(|c| c.as_ref()).find(|c| c.id == id)
    }

    fn revoke_recursive(&mut self, target_id: u128) {
        for cap in self.caps.iter_mut().flatten() {
            if cap.id == target_id { cap.active = false; }
        }
        let mut children = [0u128; 64];
        let mut child_count = 0;
        for cap in self.caps.iter().filter_map(|c| c.as_ref()) {
            if cap.parent == Some(target_id) && cap.active && child_count < 64 {
                children[child_count] = cap.id;
                child_count += 1;
            }
        }
        for i in 0..child_count {
            self.revoke_recursive(children[i]);
        }
    }
}

/// Create a capability for a WASM module (delegates from root internally).
pub fn create_module_cap(rights: Rights, ttl_ticks: Option<u64>) -> Result<u128, CapError> {
    let root = *ROOT_CAP.lock();
    VAULT.lock().create(root, ResourceKind::Execute, rights, ttl_ticks)
}

/// Check a capability against the global vault.
pub fn check_global(cap_id: u128, required: Rights) -> Result<(), CapError> {
    VAULT.lock().check(cap_id, required).map(|_| ())
}

/// Short hex representation of a 128-bit cap ID (first 8 hex chars)
pub fn short_id(id: u128) -> u32 {
    (id >> 96) as u32
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

// === PRNG: xorshift128+ seeded from TSC ===

struct Xorshift128 {
    s0: u64,
    s1: u64,
}

impl Xorshift128 {
    const fn new() -> Self {
        Xorshift128 { s0: 1, s1: 1 } // Non-zero placeholder, re-seeded in init
    }

    fn next_u128(&mut self) -> u128 {
        let mut s1 = self.s0;
        let s0 = self.s1;
        self.s0 = s0;
        s1 ^= s1 << 23;
        s1 ^= s1 >> 17;
        s1 ^= s0;
        s1 ^= s0 >> 26;
        self.s1 = s1;
        let result = s0.wrapping_add(s1);
        ((s0 as u128) << 64) | (result as u128)
    }
}

fn seed_prng() {
    let mut prng = PRNG.lock();
    let tsc1 = rdtsc();
    let tsc2 = rdtsc();
    // Mix with constants to avoid zero state
    prng.s0 = tsc1 ^ 0x6A09E667F3BCC908; // Fractional part of sqrt(2)
    prng.s1 = tsc2 ^ 0xBB67AE8584CAA73B; // Fractional part of sqrt(3)
    if prng.s0 == 0 { prng.s0 = 1; }
    if prng.s1 == 0 { prng.s1 = 1; }
    // Warm up: discard first few outputs
    for _ in 0..16 { prng.next_u128(); }
}

fn next_id() -> u128 {
    let mut prng = PRNG.lock();
    loop {
        let id = prng.next_u128();
        if id != 0 { return id; } // Zero is reserved as "no capability"
    }
}

fn rdtsc() -> u64 {
    let lo: u32;
    let hi: u32;
    // SAFETY: rdtsc is always available on x86_64, side-effect-free
    unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi); }
    ((hi as u64) << 32) | (lo as u64)
}
