//! KVM steal time (`MSR_KVM_STEAL_TIME`; KVM `record_steal_time`,
//! `kvm_steal_time_set_preempted`). A vCPU shares its host core with fibers
//! and leaves it, still runnable, whenever one of them is due. The record
//! tells the guest how long each vCPU sat runnable off its core (`steal`) and
//! which vCPUs are off it right now (`preempted`). Linux's TLB shootdown
//! (`kvm_flush_tlb_multi`) reads the second: a preempted vCPU gets
//! `KVM_VCPU_FLUSH_TLB` in its record instead of an IPI the sender would wait
//! on, and the host flushes that vCPU's TLB before its next entry.

use crate::microvm::devices::guest_mem::GuestMem;
use core::sync::atomic::{AtomicBool, Ordering, fence};

pub const MSR_KVM_STEAL_TIME: u32 = 0x4b56_4d03;
const KVM_MSR_ENABLED: u64 = 1;
/// `KVM_STEAL_RESERVED_MASK`; the record is 64-byte aligned.
const RESERVED: u64 = 0x3e;
const ADDR_MASK: u64 = !0x3f;
// `struct kvm_steal_time`.
const OFF_STEAL: u64 = 0;
const OFF_VERSION: u64 = 8;
const OFF_PREEMPTED: u64 = 16;
const KVM_VCPU_PREEMPTED: u8 = 1;
const KVM_VCPU_FLUSH_TLB: u8 = 2;

/// Announced for this launch (`arm`).
static ON: AtomicBool = AtomicBool::new(false);

/// `set microvm_steal off` keeps the old contract: no steal time, and the
/// `KVM_HINTS_REALTIME` hint instead.
pub fn configured() -> bool {
    !crate::config::get("microvm_steal").is_some_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// Decide once per launch, before the guest reads CPUID.
pub fn arm() {
    let on = configured();
    ON.store(on, Ordering::Release);
    crate::kprintln!("[microvm] steal time + PV TLB flush: {}", if on { "on" } else { "off (config)" });
}

pub fn enabled() -> bool {
    ON.load(Ordering::Acquire)
}

pub struct StealTime {
    /// The MSR as the guest wrote it.
    msr: u64,
    /// Steal in host TSC ticks, not yet in the record.
    owed_tsc: u64,
    /// Host TSC when the vCPU left its core runnable; 0 = on it.
    out_since: u64,
    /// The record holds `KVM_VCPU_PREEMPTED` (KVM `vcpu->arch.st.preempted`).
    preempted: bool,
    /// Update the record before the next entry (`KVM_REQ_STEAL_UPDATE`).
    update: bool,
}

impl StealTime {
    pub const fn new() -> Self {
        StealTime { msr: 0, owed_tsc: 0, out_since: 0, preempted: false, update: false }
    }

    fn gpa(&self) -> Option<u64> {
        (self.msr & KVM_MSR_ENABLED != 0).then_some(self.msr & ADDR_MASK)
    }

    /// `kvm_get_msr_common` / `kvm_set_msr_common` for `MSR_KVM_STEAL_TIME`.
    pub fn msr(&mut self, write: Option<u64>) -> Result<u64, ()> {
        if !enabled() {
            return Err(());
        }
        let Some(v) = write else { return Ok(self.msr) };
        if v & RESERVED != 0 {
            return Err(());
        }
        self.msr = v;
        self.preempted = false;
        self.update = v & KVM_MSR_ENABLED != 0;
        Ok(0)
    }

    /// The vCPU gives up its core while runnable (`kvm_arch_vcpu_put` with
    /// `vcpu->preempted`).
    pub fn leave(&mut self, mem: &GuestMem) {
        let Some(gpa) = self.gpa() else { return };
        self.out_since = crate::interrupts::rdtsc();
        if !self.preempted && mem.write_u8(gpa + OFF_PREEMPTED, KVM_VCPU_PREEMPTED) {
            self.preempted = true;
        }
    }

    /// Back on its core (`kvm_arch_vcpu_load`).
    pub fn back(&mut self) {
        if self.out_since == 0 {
            return;
        }
        self.owed_tsc += crate::interrupts::rdtsc().wrapping_sub(self.out_since);
        self.out_since = 0;
        self.update = true;
    }

    /// `record_steal_time`, before the entry. True: a sender deferred a TLB
    /// flush to this vCPU while it was out.
    pub fn record(&mut self, mem: &GuestMem) -> bool {
        if !self.update {
            return false;
        }
        self.update = false;
        let Some(gpa) = self.gpa() else { return false };
        // Atomic against a sender's cmpxchg that adds KVM_VCPU_FLUSH_TLB.
        let flush = mem.xchg_u8(gpa + OFF_PREEMPTED, 0)
            .is_some_and(|s| s & KVM_VCPU_FLUSH_TLB != 0);
        self.preempted = false;
        let Some(mut version) = mem.read_u32(gpa + OFF_VERSION) else { return flush };
        // An odd version before the first write is junk.
        if version & 1 != 0 {
            version = version.wrapping_add(1);
        }
        version = version.wrapping_add(1);
        mem.write_u32(gpa + OFF_VERSION, version);
        fence(Ordering::Release);
        let hz = crate::interrupts::tsc_freq().max(1) as u128;
        let ns = (core::mem::take(&mut self.owed_tsc) as u128 * 1_000_000_000 / hz) as u64;
        let steal = mem.read_u64(gpa + OFF_STEAL).unwrap_or(0).wrapping_add(ns);
        mem.write_bytes(gpa + OFF_STEAL, &steal.to_le_bytes());
        fence(Ordering::Release);
        mem.write_u32(gpa + OFF_VERSION, version.wrapping_add(1));
        flush
    }
}
