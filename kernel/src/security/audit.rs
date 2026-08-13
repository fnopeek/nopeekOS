//! Audit Log
//!
//! Ring buffer of the capability operations worth keeping: create, revoke,
//! deny, expire. Oldest entries are overwritten when full.
//!
//! A capability check that PASSES is deliberately not an entry, only a
//! counter. It is the expected outcome of every gated host call, so recording
//! it wrapped the 1024-entry ring roughly a thousand times per hour — the
//! denials and grants the log exists for were gone within seconds, buried
//! under routine successes. It also put one global lock in front of every
//! host call on a six-core scheduler. Counting keeps the number and gives the
//! ring back to the events that carry information.

use core::sync::atomic::{AtomicU64, Ordering};

use alloc::vec::Vec;
use spin::Mutex;
use crate::interrupts;
use crate::capability::CapId;

const MAX_ENTRIES: usize = 1024;

static LOG: Mutex<AuditLog> = Mutex::new(AuditLog::new());
/// Capability checks that passed. Relaxed: a running total nobody orders
/// against, on the hottest path in the system.
static CHECKS_PASSED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum AuditOp {
    Create { parent_id: CapId, new_id: CapId },
    Revoke { revoker_id: CapId, target_id: CapId },
    Denied { reason: DenyReason },
    Expired { cap_id: CapId },
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
pub enum DenyReason {
    NotFound,
    Revoked,
    Expired,
    InsufficientRights,
    EscalationAttempt,
    VaultFull,
}

#[derive(Debug, Clone, Copy)]
pub struct AuditEntry {
    pub tick: u64,
    pub op: AuditOp,
}

struct AuditLog {
    entries: Option<Vec<AuditEntry>>,
    write_pos: usize,
    total_count: u64,
}

impl AuditLog {
    const fn new() -> Self {
        AuditLog { entries: None, write_pos: 0, total_count: 0 }
    }

    fn ensure_init(&mut self) {
        if self.entries.is_none() {
            let mut v = Vec::with_capacity(MAX_ENTRIES);
            // Filler for the unwritten slots. `recent()` never returns them:
            // it bounds the read by `total_count`.
            v.resize(MAX_ENTRIES, AuditEntry {
                tick: 0,
                op: AuditOp::Denied { reason: DenyReason::NotFound },
            });
            self.entries = Some(v);
        }
    }

    fn record(&mut self, op: AuditOp) {
        self.ensure_init();
        let entry = AuditEntry { tick: interrupts::ticks(), op };
        if let Some(entries) = &mut self.entries {
            entries[self.write_pos] = entry;
            self.write_pos = (self.write_pos + 1) % MAX_ENTRIES;
            self.total_count += 1;
        }
    }

    fn recent(&self, count: usize) -> Vec<AuditEntry> {
        let mut result = Vec::new();
        if let Some(entries) = &self.entries {
            let stored = (self.total_count as usize).min(MAX_ENTRIES);
            let n = count.min(stored);
            for i in 0..n {
                let idx = if self.write_pos >= n {
                    self.write_pos - n + i
                } else {
                    (MAX_ENTRIES + self.write_pos - n + i) % MAX_ENTRIES
                };
                result.push(entries[idx]);
            }
        }
        result
    }
}

pub fn record(op: AuditOp) {
    LOG.lock().record(op);
}

/// A capability check passed. Counted, not recorded — see the module header.
pub fn record_check_passed() {
    CHECKS_PASSED.fetch_add(1, Ordering::Relaxed);
}

pub fn checks_passed() -> u64 {
    CHECKS_PASSED.load(Ordering::Relaxed)
}

pub fn recent(count: usize) -> Vec<AuditEntry> {
    LOG.lock().recent(count)
}

pub fn total_count() -> u64 {
    LOG.lock().total_count
}
