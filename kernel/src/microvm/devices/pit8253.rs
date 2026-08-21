//! i8253/i8254 channel 0 — the guest's jiffy source.
//!
//! Ported against the driver the guest actually runs,
//! `arch/x86/kernel/i8253.c` + `drivers/clocksource/i8253.c`:
//!
//!   * `pit_shutdown`    → 0x43 ← 0x30 (mode 0), then counter 0,0
//!   * `pit_set_periodic`→ 0x43 ← 0x34 (mode 2), then LATCH lo,hi
//!   * `pit_set_oneshot` → 0x43 ← 0x38 (mode 4)
//!   * `pit_next_event`  → counter lo,hi (mode already 4)
//!
//! Why this exists at all: the old emulation kept a single `pit_enabled`
//! bool and no reload value, so it had no PERIOD — it was paced at a
//! hardcoded 1 kHz off the host TSC to "MATCH the LAPIC timer's programmed
//! rate". That guess is the thing Linux checks. `calibrate_APIC_clock()`
//! runs the LAPIC timer periodically and counts JIFFIES against it,
//! requiring one LAPIC tick per jiffy within ±2 over the run. Two sources
//! paced by two different rules only hold that ratio by luck, and on this
//! hardware the luck runs out: the guest prints
//!
//!     APIC timer disabled due to verification failure
//!     Clockevents: could not switch to one-shot mode: lapic is not functional
//!     Could not switch to high resolution mode on CPU 0 … 5
//!
//! after which it has no hrtimers on any CPU — and TCP pacing, TSQ, TLP and
//! RACK all hang off hrtimers. A guest that renders fine and sends two
//! frames in three seconds looks exactly like that.
//!
//! So: real reload tracking, a period derived from it, and — the part that
//! makes the ratio hold regardless of how often the vCPU gets to run — a
//! BACKLOG. A missed tick is owed, not lost.

use crate::interrupts::rdtsc;

/// The i8254 input clock, 1.193182 MHz (`PIT_TICK_RATE`, i8253.h).
const PIT_HZ: u64 = 1_193_182;

/// Never deliver two guest timer ticks closer together than this.
///
/// Shared with the LAPIC timer ON PURPOSE and referenced from there: the two
/// sources must be capped identically or their delivered rates diverge and
/// Linux's calibration verification fails. The cap paces DELIVERY; what cannot
/// be delivered now is owed (see `pending`), so the ratio survives the cap.
pub const MIN_TICK_GAP_HZ: u64 = 1000;

/// How many owed ticks we are willing to carry.
///
/// Not unbounded: after a long stall, repaying thousands of ticks would be an
/// interrupt storm that starves everything else — and TCP pacing arms bursts of
/// ~40 µs one-shots at connection setup, which is the exact storm the old code's
/// hard 1 kHz drop-cap was written to prevent. Carrying a few keeps the ratio
/// through ordinary scheduling jitter; dropping the rest keeps the guest's
/// wall-clock honest, because Linux reads time from the TSC, not from counting
/// interrupts.
pub const MAX_PENDING: u32 = 8;

/// Channel-0 access mode (0x43 bits 5:4).
const ACCESS_LATCH: u8 = 0;
const ACCESS_LO: u8 = 1;
const ACCESS_HI: u8 = 2;
const ACCESS_LOHI: u8 = 3;

pub struct Pit {
    /// Reload value written to port 0x40. 0 means 65536 (i8254 wraps).
    reload: u16,
    /// Operating mode, 0x43 bits 3:1. Linux uses 0 (shutdown), 2 (periodic)
    /// and 4 (one-shot / software strobe).
    mode: u8,
    /// Access mode, 0x43 bits 5:4.
    access: u8,
    /// Low byte held between the two writes of an ACCESS_LOHI pair.
    half: Option<u8>,
    /// Host TSC the current count started from.
    start_tsc: u64,
    /// Host TSC of the last delivered tick — the delivery pacer.
    last_fire_tsc: u64,
    /// Ticks owed to the guest.
    pending: u32,
    /// One-shot latch: mode 0/4 fire once per counter write.
    fired: bool,
}

impl Pit {
    pub const fn new() -> Self {
        // Linux's first act is to program the channel, so the boot defaults
        // only have to be harmless: periodic at the classic 100 Hz divisor,
        // live, so a guest that never touches the PIT still gets a tick.
        Pit {
            reload: 11932,
            mode: 2,
            access: ACCESS_LOHI,
            half: None,
            start_tsc: 0,
            last_fire_tsc: 0,
            pending: 0,
            fired: false,
        }
    }

    /// Port 0x43 write (mode/command). Only channel 0 (bits 7:6 == 0) matters;
    /// a latch command (access bits == 0) reads the counter and does not change
    /// the mode.
    pub fn command(&mut self, v: u8) {
        if (v >> 6) & 0x3 != 0 {
            return; // channel 1/2 or read-back — not our tick
        }
        let access = (v >> 4) & 0x3;
        if access == ACCESS_LATCH {
            return;
        }
        self.access = access;
        self.mode = (v >> 1) & 0x7;
        self.half = None;
        // A mode write re-arms: the counter is reloaded on the next data write.
        self.fired = false;
        self.pending = 0;
    }

    /// Port 0x40 write (channel-0 counter). Completes the reload for the
    /// current access mode and restarts the count.
    pub fn write_counter(&mut self, v: u8) {
        match self.access {
            ACCESS_LO => self.set_reload(u16::from(v)),
            ACCESS_HI => self.set_reload(u16::from(v) << 8),
            _ => match self.half.take() {
                None => self.half = Some(v),
                Some(lo) => self.set_reload(u16::from(lo) | (u16::from(v) << 8)),
            },
        }
    }

    fn set_reload(&mut self, r: u16) {
        self.reload = r;
        self.start_tsc = rdtsc();
        self.fired = false;
        self.pending = 0;
    }

    /// Is channel 0 producing a tick at all? Mode 0 is how `pit_shutdown`
    /// turns it off; a zero reload after that leaves nothing to count.
    pub fn live(&self) -> bool {
        self.mode != 0
    }

    /// Period of one full count, in host TSC cycles. `reload == 0` is 65536.
    fn period_tsc(&self) -> u64 {
        let count = if self.reload == 0 { 65536u64 } else { u64::from(self.reload) };
        count.saturating_mul(crate::interrupts::tsc_freq()) / PIT_HZ
    }

    /// Accrue whatever the guest is owed and hand back ONE tick if the delivery
    /// pacer allows it. Mirrors `LocalApic::timer_due` exactly — same accrual,
    /// same cap, same latch — because "exactly" is the whole point: these two
    /// clocks are compared against each other by the guest.
    pub fn due(&mut self) -> bool {
        if !self.live() {
            return false;
        }
        let period = self.period_tsc();
        if period == 0 {
            return false;
        }
        let now = rdtsc();
        if self.start_tsc == 0 {
            self.start_tsc = now;
        }
        let elapsed = now.saturating_sub(self.start_tsc);
        let periods = elapsed / period;
        if periods > 0 {
            self.start_tsc = self.start_tsc.wrapping_add(periods * period);
            // One-shot (mode 0/4) owes exactly one tick per counter write.
            let owed = if self.mode == 2 || self.mode == 3 {
                periods.min(u64::from(MAX_PENDING)) as u32
            } else if !self.fired {
                self.fired = true;
                1
            } else {
                0
            };
            self.pending = (self.pending + owed).min(MAX_PENDING);
        }
        if self.pending == 0 {
            return false;
        }
        let gap = (crate::interrupts::tsc_freq() / MIN_TICK_GAP_HZ).max(1);
        if now.wrapping_sub(self.last_fire_tsc) < gap {
            return false;
        }
        self.last_fire_tsc = now;
        self.pending -= 1;
        true
    }

    /// Would `due()` fire right now? Non-consuming — for the yield/park checks,
    /// which must not eat a tick the inject site is about to deliver. The LAPIC
    /// learned this the hard way; see `lapic::timer_pending`.
    pub fn pending_now(&self) -> bool {
        if !self.live() {
            return false;
        }
        let period = self.period_tsc();
        if period == 0 {
            return false;
        }
        let now = rdtsc();
        let gap = (crate::interrupts::tsc_freq() / MIN_TICK_GAP_HZ).max(1);
        if now.wrapping_sub(self.last_fire_tsc) < gap {
            return false;
        }
        if self.pending > 0 {
            return true;
        }
        let start = if self.start_tsc == 0 { now } else { self.start_tsc };
        now.saturating_sub(start) >= period && (self.mode == 2 || self.mode == 3 || !self.fired)
    }
}
