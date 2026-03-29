//! Audit Log
//!
//! Immutable record of every capability operation.
//! Ring buffer — oldest entries are overwritten when full.
//! Every create, revoke, check, and deny is logged.

use alloc::vec::Vec;
use spin::Mutex;
use crate::interrupts;

const MAX_ENTRIES: usize = 1024;

static LOG: Mutex<AuditLog> = Mutex::new(AuditLog::new());

#[derive(Debug, Clone, Copy)]
pub enum AuditOp {
    Create { parent_id: u128, new_id: u128 },
    Revoke { revoker_id: u128, target_id: u128 },
    Check { cap_id: u128 },
    Denied { reason: DenyReason },
    Expired { cap_id: u128 },
}

#[derive(Debug, Clone, Copy)]
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
            v.resize(MAX_ENTRIES, AuditEntry { tick: 0, op: AuditOp::Check { cap_id: 0 } });
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

pub fn recent(count: usize) -> Vec<AuditEntry> {
    LOG.lock().recent(count)
}

pub fn total_count() -> u64 {
    LOG.lock().total_count
}
