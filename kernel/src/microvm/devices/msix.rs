//! MSI-X table of one emulated PCI function (PCI 3.0 §6.8.2).
//!
//! The table lives in atomics, not in the device: a backend worker fires a
//! queue's vector straight into the target vCPU's LAPIC (posted + kick) —
//! KVM's irqfd → MSI route — without the device lock and without the BSP.
//! Table and PBA sit in BAR0 at `TABLE_OFF` / `PBA_OFF`.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// BAR0 offsets of the table (16 bytes per entry) and the pending-bit array.
pub const TABLE_OFF: u32 = 0x2000;
pub const PBA_OFF: u32 = 0x3000;
const ENTRY_MASKED: u32 = 1;
/// PCI capability ID of MSI-X.
pub const CAP_ID: u32 = 0x11;

/// Whether devices offer MSI-X this run: on unless `set microvm_msix off`.
pub fn wanted() -> bool {
    !crate::config::get("microvm_msix").is_some_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

pub struct Msix<const N: usize> {
    offered: AtomicBool,
    enabled: AtomicBool,
    func_mask: AtomicBool,
    addr: [AtomicU64; N],
    data: [AtomicU32; N],
    /// Vector control; every entry starts masked (PCI spec).
    ctrl: [AtomicU32; N],
    pending: AtomicU32,
    /// `cores` probe: messages sent, messages latched while masked.
    sent: AtomicU64,
    latched: AtomicU64,
}

impl<const N: usize> Msix<N> {
    pub const fn new() -> Self {
        Self {
            offered: AtomicBool::new(false),
            enabled: AtomicBool::new(false),
            func_mask: AtomicBool::new(false),
            addr: [const { AtomicU64::new(0) }; N],
            data: [const { AtomicU32::new(0) }; N],
            ctrl: [const { AtomicU32::new(ENTRY_MASKED) }; N],
            pending: AtomicU32::new(0),
            sent: AtomicU64::new(0),
            latched: AtomicU64::new(0),
        }
    }

    /// Power-on state at VM open; `offered` decides whether the capability
    /// appears in config space at all.
    pub fn reset(&self, offered: bool) {
        self.offered.store(offered, Ordering::Release);
        self.enabled.store(false, Ordering::Release);
        self.func_mask.store(false, Ordering::Release);
        for i in 0..N {
            self.addr[i].store(0, Ordering::Relaxed);
            self.data[i].store(0, Ordering::Relaxed);
            self.ctrl[i].store(ENTRY_MASKED, Ordering::Relaxed);
        }
        self.pending.store(0, Ordering::Release);
    }

    pub fn offered(&self) -> bool { self.offered.load(Ordering::Acquire) }

    /// The guest switched MSI-X on: INTx is silent, every interrupt is a message.
    pub fn active(&self) -> bool {
        self.offered() && self.enabled.load(Ordering::Acquire)
    }

    /// Message control word of the capability (read side).
    pub fn msgctl(&self) -> u16 {
        (N as u16 - 1)
            | if self.enabled.load(Ordering::Acquire) { 1 << 15 } else { 0 }
            | if self.func_mask.load(Ordering::Acquire) { 1 << 14 } else { 0 }
    }

    /// Guest wrote the message control word: enable / function mask.
    pub fn set_msgctl(&self, v: u16) {
        if !self.offered() { return; }
        self.enabled.store(v & (1 << 15) != 0, Ordering::Release);
        self.func_mask.store(v & (1 << 14) != 0, Ordering::Release);
        self.flush_pending();
    }

    /// Signal `vector` (a virtio `msix_vector`). `false` = MSI-X is off, the
    /// caller raises INTx. With MSI-X on, 0xFFFF (no vector) is no interrupt.
    pub fn notify(&self, vector: u16) -> bool {
        if !self.active() { return false; }
        if (vector as usize) < N { self.fire(vector as usize); }
        true
    }

    /// Send vector `v`'s message, or latch it in the PBA while it is masked.
    fn fire(&self, v: usize) {
        if self.func_mask.load(Ordering::Acquire)
            || self.ctrl[v].load(Ordering::Acquire) & ENTRY_MASKED != 0
        {
            self.pending.fetch_or(1 << v, Ordering::AcqRel);
            self.latched.fetch_add(1, Ordering::Relaxed);
            return;
        }
        self.sent.fetch_add(1, Ordering::Relaxed);
        // Message address: 0xFEE, destination ID in bits 19:12, destination mode
        // (logical) in bit 2. Data: vector in 7:0, delivery mode in 10:8.
        let addr = self.addr[v].load(Ordering::Acquire);
        let data = self.data[v].load(Ordering::Acquire);
        crate::microvm::cpu::svm::lapic::deliver_msg(
            (addr >> 12) as u8,
            addr & (1 << 2) != 0,
            ((data >> 8) & 0x7) as u8,
            data as u8,
            crate::microvm::cpu::svm::lapic::vcpu_on_this_core(),
        );
    }

    /// Unmasked entries with a pending bit fire now (PCI: on unmask).
    fn flush_pending(&self) {
        if !self.enabled.load(Ordering::Acquire) || self.func_mask.load(Ordering::Acquire) { return; }
        for v in 0..N {
            if self.ctrl[v].load(Ordering::Acquire) & ENTRY_MASKED == 0
                && self.pending.fetch_and(!(1 << v), Ordering::AcqRel) & (1 << v) != 0
            {
                self.fire(v);
            }
        }
    }

    /// MMIO into the table / PBA (BAR0-relative `off`). `Some` if handled.
    pub fn mmio(&self, off: u32, write: Option<u32>) -> Option<u32> {
        if !self.offered() { return None; }
        if (TABLE_OFF..TABLE_OFF + 16 * N as u32).contains(&off) {
            let rel = off - TABLE_OFF;
            let v = (rel / 16) as usize;
            return Some(match (rel % 16, write) {
                (0, None) => self.addr[v].load(Ordering::Acquire) as u32,
                (4, None) => (self.addr[v].load(Ordering::Acquire) >> 32) as u32,
                (8, None) => self.data[v].load(Ordering::Acquire),
                (12, None) => self.ctrl[v].load(Ordering::Acquire),
                (0, Some(x)) => {
                    let a = self.addr[v].load(Ordering::Acquire);
                    self.addr[v].store((a & !0xFFFF_FFFF) | x as u64, Ordering::Release); 0
                }
                (4, Some(x)) => {
                    let a = self.addr[v].load(Ordering::Acquire);
                    self.addr[v].store((a & 0xFFFF_FFFF) | (x as u64) << 32, Ordering::Release); 0
                }
                (8, Some(x)) => { self.data[v].store(x, Ordering::Release); 0 }
                (12, Some(x)) => {
                    self.ctrl[v].store(x & ENTRY_MASKED, Ordering::Release);
                    self.flush_pending();
                    0
                }
                _ => 0,
            });
        }
        if (PBA_OFF..PBA_OFF + 8).contains(&off) {
            return Some(if write.is_none() && off == PBA_OFF {
                self.pending.load(Ordering::Acquire)
            } else { 0 });
        }
        None
    }

    /// One line of state for `cores`: [sent, latched] and the table.
    pub fn report(&self) -> Option<([u64; 2], alloc::string::String)> {
        if !self.offered() { return None; }
        use core::fmt::Write;
        let mut s = alloc::string::String::new();
        let _ = write!(s, "en={} fmask={} pba={:#x}",
            self.enabled.load(Ordering::Relaxed) as u8,
            self.func_mask.load(Ordering::Relaxed) as u8,
            self.pending.load(Ordering::Relaxed));
        for v in 0..N {
            let _ = write!(s, " v{}={:#x}/{:#x}/{}", v,
                self.addr[v].load(Ordering::Relaxed), self.data[v].load(Ordering::Relaxed),
                self.ctrl[v].load(Ordering::Relaxed) & 1);
        }
        Some(([self.sent.load(Ordering::Relaxed), self.latched.load(Ordering::Relaxed)], s))
    }
}
