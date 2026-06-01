//! Minimal trap-and-emulate local APIC (xAPIC, MMIO @ 0xFEE00000) for
//! the microvm guest. Per-vCPU.
//!
//! Scope (guest-SMP Stage 1): enough for a UP Linux 6.18 guest booted
//! `noapic acpi=off` WITHOUT `nolapic` to bring the LAPIC up
//! (`setup_local_APIC`), calibrate + run the LAPIC timer, and EOI. ICR
//! writes are stored (IPI / INIT-SIPI decode lands in Stage 3 for
//! multi-vCPU). Device IRQs still flow through the 8259 PIC.
//!
//! Semantics are ported 1:1 from the kernel source the guest runs
//! against (`~/.cache/nopeekos/linux-src/linux-6.18.26`):
//!   * register offsets / bits — `arch/x86/include/asm/apicdef.h`
//!   * read/write rules + timer — `arch/x86/kvm/lapic.c`
//!     (`kvm_lapic_reg_write`, `apic_get_tmcct`, `start_apic_timer`)
//!   * bring-up sequence — `arch/x86/kernel/apic/apic.c`
//!     (`setup_local_APIC`, `calibrate_APIC_clock`)
//!
//! The register file is a flat `[u32; 64]` indexed by `offset >> 4`,
//! mirroring KVM's regs page — most accesses are plain store/read-back;
//! only SPIV-enable, the timer (TMICT/TMCCT/TDCR/LVTT), EOI and ESR have
//! behaviour.

use crate::interrupts::rdtsc;

/// Guest-physical base of the LAPIC MMIO page (architectural default,
/// `APIC_DEFAULT_PHYS_BASE`). One 4 KiB page; registers live in the
/// first 1 KiB at 16-byte stride.
pub const LAPIC_BASE: u64 = 0xFEE0_0000;
pub const LAPIC_SIZE: u64 = 0x1000;

/// IA32_APIC_BASE MSR (0x1B) value we report: base + global-enable
/// (bit 11) + BSP (bit 8). `MSR_IA32_APICBASE_*` in msr-index.h.
pub const APIC_BASE_MSR_VALUE: u64 = LAPIC_BASE | (1 << 11) | (1 << 8);

// Register offsets (apicdef.h). Index into `regs` is `off >> 4`.
const APIC_ID: u32 = 0x20;
const APIC_LVR: u32 = 0x30;
const APIC_TASKPRI: u32 = 0x80;
const APIC_EOI: u32 = 0xB0;
const APIC_SPIV: u32 = 0xF0;
const APIC_ESR: u32 = 0x280;
const APIC_ICR: u32 = 0x300;
const APIC_ICR2: u32 = 0x310;
const APIC_LVTT: u32 = 0x320;
const APIC_TMICT: u32 = 0x380;
const APIC_TMCCT: u32 = 0x390;
const APIC_TDCR: u32 = 0x3E0;

// Bits (apicdef.h).
const SPIV_APIC_ENABLED: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
const ICR_BUSY: u32 = 1 << 12;
const VECTOR_MASK: u32 = 0xFF;

/// LVR: integrated xAPIC, version 0x14, MAXLVT = nr_lvt_entries-1 = 5
/// (6 entries: timer, thermal, perf, lint0, lint1, error), matching
/// KVM `kvm_apic_set_version` with no CMCI. `lapic_get_maxlvt()` reads
/// bits 23:16; >3 enables the ESR write-clear dance — we want that.
const LVR_VALUE: u32 = 0x14 | (5 << 16);

#[inline]
fn idx(off: u32) -> usize {
    (off >> 4) as usize
}

/// One vCPU's local APIC.
pub struct LocalApic {
    /// Flat register file indexed by `off >> 4`. Sized for the whole
    /// 4 KiB MMIO page (256 × 16-byte slots) so ANY in-page offset the
    /// guest touches — incl. AMD extended-APIC regs ≥ 0x400 — is a safe
    /// store/read-back rather than an out-of-bounds host panic. We don't
    /// advertise extended space (LVR bit 31 = 0) so Linux shouldn't go
    /// there, but bound it defensively.
    regs: [u32; 256],
    /// Host TSC captured when APIC_TMICT was last written — the timer's
    /// count-down origin. Mirrors KVM `lapic_timer.target_expiration`'s
    /// role, expressed in host TSC instead of ktime.
    timer_start_tsc: u64,
}

impl LocalApic {
    pub fn new() -> Self {
        let mut regs = [0u32; 256];
        regs[idx(APIC_LVR)] = LVR_VALUE;
        // Reset state: APIC software-disabled, spurious vector 0xFF,
        // all LVTs masked (Linux clears + re-enables in setup_local_APIC).
        regs[idx(APIC_LVTT)] = LVT_MASKED;
        LocalApic { regs, timer_start_tsc: 0 }
    }

    /// True once Linux has software-enabled the APIC (SPIV bit 8).
    #[inline]
    fn enabled(&self) -> bool {
        self.regs[idx(APIC_SPIV)] & SPIV_APIC_ENABLED != 0
    }

    /// APIC_TDCR → divide count. KVM `update_divide_count`:
    /// shift = ((tdcr&3) | ((tdcr&8)>>1)) + 1; div = 1 << (shift & 7).
    fn divide_count(&self) -> u64 {
        let tdcr = self.regs[idx(APIC_TDCR)] & 0xF;
        let shift = ((tdcr & 0x3) | ((tdcr & 0x8) >> 1)) + 1;
        1u64 << (shift & 0x7)
    }

    /// Current timer count (APIC_TMCCT), computed from elapsed host TSC
    /// since TMICT was armed. Mirrors KVM `apic_get_tmcct`: counts down
    /// from TMICT, wraps each period in periodic mode, floors at 0 in
    /// one-shot. APIC bus cycle == 1 host TSC cycle, so one timer tick =
    /// `divide_count` TSC cycles (Linux calibrates the absolute rate, so
    /// the chosen bus rate only needs to be self-consistent).
    fn tmcct(&self) -> u32 {
        let tmict = self.regs[idx(APIC_TMICT)];
        if tmict == 0 {
            return 0;
        }
        let elapsed_ticks = rdtsc().saturating_sub(self.timer_start_tsc) / self.divide_count();
        let periodic = self.regs[idx(APIC_LVTT)] & LVT_TIMER_PERIODIC != 0;
        if periodic {
            (tmict as u64 - (elapsed_ticks % tmict as u64)) as u32
        } else if elapsed_ticks >= tmict as u64 {
            0
        } else {
            (tmict as u64 - elapsed_ticks) as u32
        }
    }

    /// Read a register (32-bit). `off` is the page offset (gpa - base).
    pub fn read(&self, off: u32) -> u32 {
        match off & 0xFF0 {
            APIC_TMCCT => self.tmcct(),
            o => self.regs[idx(o)],
        }
    }

    /// Write a register (32-bit). Side-effects per KVM
    /// `kvm_lapic_reg_write`.
    pub fn write(&mut self, off: u32, val: u32) {
        match off & 0xFF0 {
            // EOI is write-only; it clears the highest in-service vector.
            // We deliver one vector at a time via VMCB EVENTINJ and don't
            // model ISR/IRR/PPR (all read 0 → accept-all), so EOI is a
            // no-op — the guest's ack just returns cleanly.
            APIC_EOI => {}
            // APIC_ID: xAPIC id in bits 31:24. Store (UP guest keeps 0).
            APIC_ID => self.regs[idx(APIC_ID)] = val,
            APIC_TASKPRI => self.regs[idx(APIC_TASKPRI)] = val & 0xFF,
            APIC_SPIV => self.regs[idx(APIC_SPIV)] = val & 0x3FF,
            // ESR is write-to-clear on integrated APICs.
            APIC_ESR => self.regs[idx(APIC_ESR)] = 0,
            // TMICT write (re)arms the timer from `now`. Ignored count 0
            // disarms (tmcct returns 0).
            APIC_TMICT => {
                self.regs[idx(APIC_TMICT)] = val;
                self.timer_start_tsc = rdtsc();
            }
            APIC_TDCR => self.regs[idx(APIC_TDCR)] = val & 0xB,
            // ICR low: clear BUSY, store. IPI / INIT-SIPI delivery is a
            // Stage-3 (multi-vCPU) concern — a UP guest in virtual-wire
            // mode does not send IPIs during bring-up.
            APIC_ICR => self.regs[idx(APIC_ICR)] = val & !ICR_BUSY,
            APIC_ICR2 => self.regs[idx(APIC_ICR2)] = val & 0xFF00_0000,
            // LVR is read-only.
            APIC_LVR => {}
            // Everything else (LVTT/LVT0/LVT1/LVTERR/LDR/DFR/…): store.
            o => self.regs[idx(o)] = val,
        }
    }

    /// If the LAPIC timer is the active clock source right now, the
    /// vector to inject for a tick; else `None` (caller falls back to the
    /// 8259 PIT IRQ0). Active = APIC software-enabled + LVTT unmasked +
    /// a non-zero initial count. During `calibrate_APIC_clock` LVTT is
    /// MASKED, so this returns `None` and the PIT keeps driving jiffies;
    /// once `setup_APIC_timer` unmasks it, the tick moves to the LVTT
    /// vector. We pace one tick per host 100 Hz `ticks()` exactly like
    /// the IRQ0 path (Linux's wall-clock is TSC-based; jiffies just need
    /// a steady tick).
    pub fn timer_tick_vector(&self) -> Option<u8> {
        let lvtt = self.regs[idx(APIC_LVTT)];
        if self.enabled() && lvtt & LVT_MASKED == 0 && self.regs[idx(APIC_TMICT)] != 0 {
            Some((lvtt & VECTOR_MASK) as u8)
        } else {
            None
        }
    }
}
