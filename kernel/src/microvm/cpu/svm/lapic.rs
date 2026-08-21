//! Minimal trap-and-emulate local APIC (xAPIC, MMIO @ 0xFEE00000) for
//! the microvm guest. Per-vCPU.
//!
//! Scope (guest-SMP Stage 1+2): enough for a Linux 6.18 guest booted
//! `noapic acpi=off` WITHOUT `nolapic` to bring the LAPIC up
//! (`setup_local_APIC`), calibrate + run the LAPIC timer, and EOI.
//! Stage 2: ICR writes are stored AND the INIT/SIPI delivery modes are
//! decoded + logged (the guest sends them when bringing up the AP the
//! MP-table enumerates), but the AP is NOT started — actual cross-vCPU
//! delivery (spawn the AP fiber) lands in Stage 3. Device IRQs still
//! flow through the 8259 PIC.
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
use crate::microvm::devices::pit8253::MAX_PENDING;

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

// ICR delivery-mode field (bits 10:8) — apicdef.h.
const ICR_DM_MASK: u32 = 0x700;
pub const ICR_DM_FIXED: u32 = 0x000;
pub const ICR_DM_LOWEST: u32 = 0x100;
#[allow(dead_code)] // completes the delivery-mode set next to FIXED/LOWEST
pub const ICR_DM_INIT: u32 = 0x500;
pub const ICR_DM_STARTUP: u32 = 0x600;

/// Decoded ICR write — the inter-processor interrupt a vCPU just issued.
/// The caller (which knows the sender's apic_id + the shared IPI state)
/// routes it: FIXED/LOWEST → set the target vCPU(s)' pending vector;
/// STARTUP → spawn the AP at `vector`; INIT → no-op (AP starts at SIPI).
#[derive(Clone, Copy)]
pub struct IcrWrite {
    /// Delivery mode, masked to `ICR_DM_MASK` (one of `ICR_DM_*`).
    pub delivery_mode: u32,
    /// Destination shorthand (ICR bits 19:18): 0=dest field, 1=self,
    /// 2=all-incl-self, 3=all-excl-self.
    pub shorthand: u32,
    /// Physical destination APIC id (ICR2 bits 31:24), valid when
    /// `shorthand == 0`.
    pub dest: u8,
    /// Interrupt vector (FIXED) or SIPI start page (STARTUP).
    pub vector: u8,
}

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
    /// One-shot latch: a one-shot timer fires exactly once per arm; cleared on
    /// every TMICT write (the guest re-arms for each hrtimer deadline). Periodic
    /// mode ignores this (re-arms by advancing the origin each period).
    timer_fired: bool,
    /// Host TSC of the last timer-IRQ delivery — rate-caps injection at ~1 kHz.
    /// CONFIG_HZ=1000 means the guest programs ~1 ms ticks, but during bursts of
    /// short hrtimers (TCP pacing at connection setup) it can arm ~40 µs one-
    /// shots; firing every one (~25 kHz, measured) with priority over IRQ10
    /// STARVES the network at cold start (the "first speedtest after boot is far
    /// worse" symptom). The guest tolerates 1 ms granularity (its jiffies tick),
    /// so cap delivery at ~1 kHz and let IRQ10 keep its slots.
    last_fire_tsc: u64,
    /// Ticks owed to the guest but not yet delivered.
    ///
    /// The old code computed `periods = elapsed / period`, advanced the origin
    /// by all of them, and delivered ONE — nine out of ten ticks vanished
    /// whenever the vCPU had been away for ten periods. That is invisible for a
    /// wall-clock (Linux reads the TSC) but fatal for `calibrate_APIC_clock`,
    /// which counts LAPIC ticks against JIFFIES and requires one per one. Two
    /// sources that both drop their backlog only stay in step while neither is
    /// starved; the moment one loses a slot to the other, the ratio breaks and
    /// Linux answers with "APIC timer disabled due to verification failure" —
    /// after which the guest has no hrtimers on any CPU, and TCP loses pacing,
    /// TSQ, TLP and RACK.
    ///
    /// So a missed tick is owed, not lost. Bounded, because repaying a long
    /// stall in full would be the interrupt storm the rate cap exists to stop.
    /// Mirrors `pit8253::Pit` exactly — the two are compared to each other.
    pending: u32,
}

impl LocalApic {
    /// `apic_id` is this vCPU's physical xAPIC ID (BSP = 0). It seeds the
    /// APIC_ID register (bits 31:24) so `read_apic_id` returns it — the
    /// AP's bring-up + Linux's topology depend on each vCPU reporting its
    /// own ID consistently with CPUID and the MP-table.
    pub fn new(apic_id: u8) -> Self {
        let mut regs = [0u32; 256];
        regs[idx(APIC_LVR)] = LVR_VALUE;
        regs[idx(APIC_ID)] = (apic_id as u32) << 24;
        // Reset state: APIC software-disabled, spurious vector 0xFF,
        // all LVTs masked (Linux clears + re-enables in setup_local_APIC).
        regs[idx(APIC_LVTT)] = LVT_MASKED;
        LocalApic {
            regs, timer_start_tsc: 0, timer_fired: false, last_fire_tsc: 0, pending: 0,
        }
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
    /// `kvm_lapic_reg_write`. Returns `Some(IcrWrite)` for an ICR-low
    /// write so the caller can route the IPI cross-vCPU (it knows the
    /// sender id + shared state); all other writes return `None`.
    #[must_use]
    pub fn write(&mut self, off: u32, val: u32) -> Option<IcrWrite> {
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
                self.timer_fired = false; // re-armed → one-shot may fire again
                self.pending = 0;         // …and owes nothing from the old arming
            }
            APIC_TDCR => self.regs[idx(APIC_TDCR)] = val & 0xB,
            // ICR low: clear BUSY (Linux polls it for idle), store, and
            // hand the decoded IPI back to the caller for cross-vCPU
            // routing (INIT/SIPI bring-up + FIXED reschedule/TLB/etc.).
            APIC_ICR => {
                self.regs[idx(APIC_ICR)] = val & !ICR_BUSY;
                return Some(IcrWrite {
                    delivery_mode: val & ICR_DM_MASK,
                    shorthand: (val >> 18) & 0x3,
                    dest: (self.regs[idx(APIC_ICR2)] >> 24) as u8,
                    vector: (val & VECTOR_MASK) as u8,
                });
            }
            APIC_ICR2 => self.regs[idx(APIC_ICR2)] = val & 0xFF00_0000,
            // LVR is read-only.
            APIC_LVR => {}
            // Everything else (LVTT/LVT0/LVT1/LVTERR/LDR/DFR/…): store.
            o => self.regs[idx(o)] = val,
        }
        None
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
    /// True if the programmed LAPIC timer has reached 0 since the last delivery
    /// — i.e. a tick is due NOW, at the rate the GUEST programmed (its TMICT is
    /// TSC-calibrated). The inject path delivers on this instead of pacing one
    /// tick per host 100 Hz `ticks()`: the guest is CONFIG_HZ=1000, so the old
    /// 100 Hz pacing ran its jiffies + every hrtimer (TCP pacing / delayed-ACK /
    /// RTO / NAPI) 10× slow, quantizing download bandwidth to data-per-100 Hz-
    /// tick. Periodic re-arms by advancing the TSC origin each whole period (so
    /// `tmcct` stays exact); one-shot fires once until the guest re-writes TMICT.
    pub fn timer_due(&mut self) -> bool {
        let tmict = self.regs[idx(APIC_TMICT)];
        if !self.enabled() || self.regs[idx(APIC_LVTT)] & LVT_MASKED != 0 || tmict == 0 {
            return false;
        }
        let period = (tmict as u64) * self.divide_count();
        if period == 0 {
            return false;
        }
        let now = rdtsc();
        // Accrue first: whatever periods have elapsed are OWED, even if the cap
        // below forbids delivering them this instant. Advancing the origin
        // without booking the ticks is what silently dropped nine out of ten.
        let elapsed = now.saturating_sub(self.timer_start_tsc);
        let periods = elapsed / period;
        if periods > 0 {
            self.timer_start_tsc = self.timer_start_tsc.wrapping_add(periods * period);
            let periodic = self.regs[idx(APIC_LVTT)] & LVT_TIMER_PERIODIC != 0;
            let owed = if periodic {
                periods.min(u64::from(MAX_PENDING)) as u32
            } else if !self.timer_fired {
                self.timer_fired = true;
                1
            } else {
                0
            };
            self.pending = (self.pending + owed).min(MAX_PENDING);
        }
        if self.pending == 0 {
            return false;
        }
        // Deliver at most ~1 kHz, however short the guest's programmed oneshot
        // is (TCP pacing arms ~40 µs one-shots at connection setup; firing every
        // one starves IRQ10). This paces DELIVERY — the rest stays owed.
        let gap = (crate::interrupts::tsc_freq()
            / crate::microvm::devices::pit8253::MIN_TICK_GAP_HZ).max(1);
        if now.wrapping_sub(self.last_fire_tsc) < gap {
            return false;
        }
        self.last_fire_tsc = now;
        self.pending -= 1;
        true
    }

    /// Non-consuming peek: would `timer_due()` fire RIGHT NOW? Same predicate,
    /// but WITHOUT the `last_fire_tsc`/`timer_start_tsc`/`timer_fired` mutation.
    /// The inject loop calls `timer_due()` (which consumes) at multiple yield
    /// checks before the actual LVTT inject — the first true-returning call ate
    /// the tick and the inject site then saw false → the LVTT IRQ was NEVER
    /// delivered → Linux's calibrate_APIC_clock verification starved →
    /// "APIC timer disabled" → no hrtimers. Use this peek for the yield checks;
    /// only the inject site calls `timer_due()` to consume.
    pub fn timer_pending(&self) -> bool {
        let tmict = self.regs[idx(APIC_TMICT)];
        if !self.enabled() || self.regs[idx(APIC_LVTT)] & LVT_MASKED != 0 || tmict == 0 {
            return false;
        }
        let period = (tmict as u64) * self.divide_count();
        if period == 0 {
            return false;
        }
        let now = rdtsc();
        let gap = (crate::interrupts::tsc_freq()
            / crate::microvm::devices::pit8253::MIN_TICK_GAP_HZ).max(1);
        if now.wrapping_sub(self.last_fire_tsc) < gap {
            return false;
        }
        if self.pending > 0 {
            return true; // already owed — the accrual happened in timer_due
        }
        if now.saturating_sub(self.timer_start_tsc) < period {
            return false;
        }
        // Periodic always pending when due; one-shot only until it has fired.
        self.regs[idx(APIC_LVTT)] & LVT_TIMER_PERIODIC != 0 || !self.timer_fired
    }

    /// Absolute host TSC at which the guest's LVTT will NEXT fire — the
    /// hrtimer deadline in KVM's `start_sw_timer`/`apic_timer_fn` model. The
    /// vCPU fiber parks until this instant (or an RX/TX/IPI kick), so the guest
    /// 1 kHz clock advances at its programmed rate INDEPENDENT of VMRUN, instead
    /// of being polled only while in VMRUN (the frozen-clock symptom). `None`
    /// when no timer is armed (LAPIC software-disabled, LVTT masked, TMICT=0, or
    /// a one-shot that has already fired) → the caller parks on the safety cap.
    /// Honours the same ~1 kHz `min_gap` rate-cap as `timer_due`, so the returned
    /// deadline is never earlier than the next legal delivery.
    pub fn next_timer_deadline_tsc(&self) -> Option<u64> {
        let tmict = self.regs[idx(APIC_TMICT)];
        if !self.enabled() || self.regs[idx(APIC_LVTT)] & LVT_MASKED != 0 || tmict == 0 {
            return None;
        }
        let period = (tmict as u64) * self.divide_count();
        if period == 0 {
            return None;
        }
        let now = rdtsc();
        let min_gap = (crate::interrupts::tsc_freq() / 1000).max(1);
        let gap_floor = self.last_fire_tsc.wrapping_add(min_gap);
        let periodic = self.regs[idx(APIC_LVTT)] & LVT_TIMER_PERIODIC != 0;
        let elapsed = now.saturating_sub(self.timer_start_tsc);
        let boundary = if periodic {
            if elapsed >= period {
                now // overdue: fires now (subject to the gap floor)
            } else {
                self.timer_start_tsc.wrapping_add(period)
            }
        } else if self.timer_fired {
            return None; // one-shot already delivered, no further tick
        } else {
            self.timer_start_tsc.wrapping_add(period)
        };
        Some(boundary.max(gap_floor))
    }

    pub fn timer_tick_vector(&self) -> Option<u8> {
        let lvtt = self.regs[idx(APIC_LVTT)];
        if self.enabled() && lvtt & LVT_MASKED == 0 && self.regs[idx(APIC_TMICT)] != 0 {
            Some((lvtt & VECTOR_MASK) as u8)
        } else {
            None
        }
    }
}
