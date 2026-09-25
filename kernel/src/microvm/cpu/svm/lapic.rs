//! Local APIC (xAPIC, MMIO @ 0xFEE00000), per vCPU — ported from Linux
//! `arch/x86/kvm/lapic.c`. Shared by the SVM and VMX backends.
//!
//! IRR / ISR / TMR live in the register page at their architectural offsets
//! (as in KVM's regs page), so the guest reads them like hardware. Priority
//! is TPR/PPR (`__apic_update_ppr`); `has_interrupt` / `ack_interrupt` are
//! `kvm_apic_has_interrupt` / `kvm_apic_ack_interrupt`; EOI clears the
//! highest in-service vector (`apic_set_eoi`). The timer expiring sets its
//! vector in IRR — it is an interrupt source like any other, delivered by the
//! same `inject_pending_event` and taken in the guest's own priority order.
//!
//! Interrupts from other vCPUs are posted: `post` sets a bit in the target's
//! lock-free descriptor and the target folds it into IRR before each entry
//! (`sync_pir_to_irr`, KVM's posted-interrupt model).

use crate::interrupts::rdtsc;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

pub const LAPIC_BASE: u64 = 0xFEE0_0000;
pub const LAPIC_SIZE: u64 = 0x1000;

/// IA32_APIC_BASE MSR value: base + global enable (bit 11) + BSP (bit 8).
pub const APIC_BASE_MSR_VALUE: u64 = LAPIC_BASE | (1 << 11) | (1 << 8);
const APIC_BASE_X2APIC: u64 = 1 << 10;
const APIC_BASE_ENABLE: u64 = 1 << 11;

const MSR_APIC_BASE: u32 = 0x1B;
/// x2APIC register MSRs: 0x800 + (xAPIC offset >> 4).
const MSR_X2APIC_FIRST: u32 = 0x800;
const MSR_X2APIC_LAST: u32 = 0x8FF;
const X2APIC_SELF_IPI: u32 = 0x3F0;
/// IA32_TSC_DEADLINE (armed when LVTT is in TSC-deadline mode).
const MSR_TSC_DEADLINE: u32 = 0x6E0;
/// `MSR_KVM_PV_EOI_EN` (kvm_para.h).
const MSR_KVM_PV_EOI_EN: u32 = 0x4b56_4d04;
const KVM_MSR_ENABLED: u64 = 1;

// Register offsets (apicdef.h). Index into `regs` is `off >> 4`.
const APIC_ID: u32 = 0x20;
const APIC_LVR: u32 = 0x30;
const APIC_LDR: u32 = 0xD0;
const APIC_DFR: u32 = 0xE0;
const APIC_TASKPRI: u32 = 0x80;
const APIC_PROCPRI: u32 = 0xA0;
const APIC_EOI: u32 = 0xB0;
const APIC_SPIV: u32 = 0xF0;
const APIC_ISR: u32 = 0x100;
const APIC_TMR: u32 = 0x180;
const APIC_IRR: u32 = 0x200;
const APIC_ESR: u32 = 0x280;
const APIC_ICR: u32 = 0x300;
const APIC_ICR2: u32 = 0x310;
const APIC_LVTT: u32 = 0x320;
const APIC_LVTTHMR: u32 = 0x330;
const APIC_LVTPC: u32 = 0x340;
const APIC_LVT0: u32 = 0x350;
const APIC_LVT1: u32 = 0x360;
const APIC_LVTERR: u32 = 0x370;
const APIC_TMICT: u32 = 0x380;
const APIC_TMCCT: u32 = 0x390;
const APIC_TDCR: u32 = 0x3E0;

const SPIV_APIC_ENABLED: u32 = 1 << 8;
const LVT_MASKED: u32 = 1 << 16;
const LVT_TIMER_PERIODIC: u32 = 1 << 17;
/// LVTT timer mode, bits 18:17: 00 one-shot, 01 periodic, 10 TSC-deadline.
const LVT_TIMER_MODE_MASK: u32 = 0x3 << 17;
const LVT_TIMER_TSCDEADLINE: u32 = 0x2 << 17;
const APIC_MODE_EXTINT: u32 = 0x700;
const ICR_BUSY: u32 = 1 << 12;
const VECTOR_MASK: u32 = 0xFF;

const ICR_DM_MASK: u32 = 0x700;
pub const ICR_DM_FIXED: u32 = 0x000;
pub const ICR_DM_LOWEST: u32 = 0x100;
#[allow(dead_code)]
pub const ICR_DM_INIT: u32 = 0x500;
pub const ICR_DM_STARTUP: u32 = 0x600;

/// Owed periodic ticks we carry (the PIT's reinject bound, see `pit8253`).
/// `calibrate_APIC_clock` counts LAPIC ticks against PIT jiffies one for
/// one; a tick that could not be taken yet is owed, not dropped. Repaid one
/// per acknowledgement — the guest paces the repayment, not a rate cap.
const MAX_PENDING: u32 = 8;

/// Decoded ICR write — routed by the backend (it knows the other vCPUs).
#[derive(Clone, Copy)]
pub struct IcrWrite {
    pub delivery_mode: u32,
    /// Destination shorthand (bits 19:18): 0 dest, 1 self, 2 all, 3 all-but-self.
    pub shorthand: u32,
    pub dest: u8,
    pub vector: u8,
}

/// LVR: integrated xAPIC 0x14, MAXLVT 5 (KVM `kvm_apic_set_version`, no CMCI).
const LVR_VALUE: u32 = 0x14 | (5 << 16);

#[inline]
fn idx(off: u32) -> usize { (off >> 4) as usize }

// ── Posted interrupts (other vCPUs → this one) ──

pub const MAX_VCPUS: usize = 8;

/// `struct pi_desc`'s PIR: one 256-bit request bitmap per vCPU. Any vCPU may
/// set bits; only the owner clears them (in `sync_posted`).
static PIR: [[AtomicU64; 4]; MAX_VCPUS] =
    [const { [const { AtomicU64::new(0) }; 4] }; MAX_VCPUS];

/// `pi_desc.ON`: set by the poster AFTER its PIR bit, cleared by the owner
/// BEFORE it drains PIR. Whoever flips it false→true kicks, so a post that
/// lands after a drain always kicks — deriving the edge from the PIR words
/// (load, then or) lost it when the owner drained in between, and the vector
/// then sat in PIR while the target ran in guest with no exit to fold it in.
static PIR_ON: [AtomicBool; MAX_VCPUS] = [const { AtomicBool::new(false) }; MAX_VCPUS];

/// Post `vector` to vCPU `apic_id`. True when the caller must kick the
/// target (`pi_test_and_set_on`), once per burst.
pub fn post(apic_id: u8, vector: u8) -> bool {
    let t = apic_id as usize;
    if t >= MAX_VCPUS { return false; }
    PIR[t][(vector >> 6) as usize].fetch_or(1u64 << (vector & 63), Ordering::AcqRel);
    !PIR_ON[t].swap(true, Ordering::AcqRel)
}

/// Anything posted to `apic_id` and not yet folded into its IRR?
pub fn posted_pending(apic_id: u8) -> bool {
    let t = apic_id as usize;
    t < MAX_VCPUS && PIR[t].iter().any(|w| w.load(Ordering::Acquire) != 0)
}

/// Host core each vCPU runs on (indexed by apic_id), so a sender can kick it
/// out of guest mode; `usize::MAX` = not running yet.
static VCPU_HOST_CORE: [AtomicUsize; MAX_VCPUS] =
    [const { AtomicUsize::new(usize::MAX) }; MAX_VCPUS];
/// TSC of each vCPU's last exit — a running vCPU exits ≥1 kHz, a blocked one
/// goes quiet (the PV-TLB "preempted" estimate in `rip_sample`).
static VCPU_LAST_ACTIVE: [AtomicU64; MAX_VCPUS] = [const { AtomicU64::new(0) }; MAX_VCPUS];

/// Guest LAPIC accesses by register (`cores`): each one is a trapped MMIO
/// exit with an instruction fetch + decode, so which register dominates
/// decides the cure (EOI / ICR → x2APIC or PV-EOI, TMICT → TSC deadline).
pub const ACCESS_BUCKETS: usize = 8;
pub const ACCESS_LABELS: [&str; ACCESS_BUCKETS] =
    ["eoi", "icr-w", "tmict-w", "tmcct-r", "tpr", "lvt", "other-r", "other-w"];
static ACCESS_COUNTS: [AtomicU64; ACCESS_BUCKETS] = [const { AtomicU64::new(0) }; ACCESS_BUCKETS];

fn note_access(off: u32, write: bool) {
    let b = match (off & 0xFF0, write) {
        (APIC_EOI, _) => 0,
        (APIC_ICR | APIC_ICR2, true) => 1,
        (APIC_TMICT, true) => 2,
        (APIC_TMCCT, false) => 3,
        (APIC_TASKPRI, _) => 4,
        (APIC_LVTT..=APIC_LVTERR, _) => 5,
        (_, false) => 6,
        (_, true) => 7,
    };
    ACCESS_COUNTS[b].fetch_add(1, Ordering::Relaxed);
}

pub fn access_snapshot() -> [u64; ACCESS_BUCKETS] {
    core::array::from_fn(|i| ACCESS_COUNTS[i].load(Ordering::Relaxed))
}

/// Where each vCPU's host thread is right now (`cores`): a hang shows as a
/// phase that stopped moving. Written at the run-loop landmarks only.
pub const PH_INJECT: u8 = 1;
pub const PH_GUEST: u8 = 2;
pub const PH_EXIT: u8 = 3;
pub const PH_BIGLOCK: u8 = 4;
pub const PH_HALTPOLL: u8 = 5;
pub const PH_PARK: u8 = 6;
pub const PH_YIELD: u8 = 7;
pub const PHASE_LABELS: [&str; 8] =
    ["out", "inject", "guest", "exit", "big-lock", "halt-poll", "park", "yield"];
static PHASE: [core::sync::atomic::AtomicU8; MAX_VCPUS] =
    [const { core::sync::atomic::AtomicU8::new(0) }; MAX_VCPUS];
static PHASE_TSC: [AtomicU64; MAX_VCPUS] = [const { AtomicU64::new(0) }; MAX_VCPUS];

#[inline]
pub fn phase(apic_id: u8, p: u8) {
    let i = apic_id as usize;
    if i < MAX_VCPUS {
        PHASE[i].store(p, Ordering::Relaxed);
        PHASE_TSC[i].store(rdtsc(), Ordering::Relaxed);
    }
}

/// (phase, TSC it was entered, host core) for vCPU `i`.
pub fn phase_snapshot(i: usize) -> Option<(u8, u64, usize)> {
    if i >= MAX_VCPUS { return None; }
    let core = VCPU_HOST_CORE[i].load(Ordering::Relaxed);
    if core == usize::MAX { return None; }
    Some((PHASE[i].load(Ordering::Relaxed), PHASE_TSC[i].load(Ordering::Relaxed), core))
}

/// The vCPU running on the calling host core, or 0xFF (a device worker, the
/// shell). A sender that is a vCPU never kicks its own core.
pub fn vcpu_on_this_core() -> u8 {
    let cid = crate::smp::per_core::current_core_id();
    (0..MAX_VCPUS)
        .find(|&i| VCPU_HOST_CORE[i].load(Ordering::Relaxed) == cid)
        .map_or(0xFF, |i| i as u8)
}

pub fn set_host_core(apic_id: u8, core: usize) {
    if (apic_id as usize) < MAX_VCPUS {
        VCPU_HOST_CORE[apic_id as usize].store(core, Ordering::Relaxed);
    }
}

pub fn note_exit(apic_id: u8) {
    if (apic_id as usize) < MAX_VCPUS {
        VCPU_LAST_ACTIVE[apic_id as usize].store(rdtsc(), Ordering::Relaxed);
    }
}

/// `kvm_vcpu_kick`: get vCPU `target` out of guest mode / out of its park.
fn kick(target: u8, self_core: usize) {
    let Some(slot) = VCPU_HOST_CORE.get(target as usize) else { return };
    let hc = slot.load(Ordering::Relaxed);
    if hc != usize::MAX && hc != self_core {
        crate::smp::kick_host_core(hc);
    }
}

fn preempted(target: u8) -> bool {
    let Some(slot) = VCPU_LAST_ACTIVE.get(target as usize) else { return false };
    let last = slot.load(Ordering::Relaxed);
    last == 0 || rdtsc().wrapping_sub(last) > (crate::interrupts::tsc_freq() / 1000) * 2
}

/// `kvm_irq_delivery_to_apic` for an ICR write: FIXED/LOWEST are posted to
/// the target(s) and the target is kicked on the empty→pending edge; STARTUP
/// spawns the AP; INIT/NMI/SMI are not modelled cross-vCPU.
pub fn deliver_ipi(sender: u8, icr: &IcrWrite) {
    match icr.delivery_mode {
        ICR_DM_STARTUP => crate::microvm::cpu::request_ap_spawn(icr.dest, icr.vector),
        ICR_DM_FIXED | ICR_DM_LOWEST => {
            let n = crate::microvm::cpu::guest_vcpus();
            let self_core = VCPU_HOST_CORE.get(sender as usize)
                .map_or(usize::MAX, |c| c.load(Ordering::Relaxed));
            let mut send = |t: u8| {
                if t != sender {
                    crate::microvm::cpu::rip_sample::note_ipi_target(preempted(t));
                }
                if post(t, icr.vector) { kick(t, self_core); }
            };
            match icr.shorthand {
                0 => send(icr.dest),
                1 => { let _ = post(sender, icr.vector); }
                2 => (0..n).for_each(&mut send),
                _ => (0..n).filter(|&t| t != sender).for_each(&mut send),
            }
        }
        _ => {}
    }
}

/// `kvm_pv_send_ipi` (KVM_HC_SEND_IPI): a bitmap of physical APIC IDs from
/// `min` (64 per word in long mode), vector + delivery mode in `icr`. Returns
/// the number of vCPUs reached, or a negative KVM error.
pub fn pv_send_ipi(sender: u8, low: u64, high: u64, min: u64, icr: u64) -> i64 {
    const APIC_DEST_LOGICAL: u64 = 1 << 11;
    const APIC_SHORT_MASK: u64 = 0x3 << 18;
    const KVM_EINVAL: i64 = 22;
    if icr & (APIC_DEST_LOGICAL | APIC_SHORT_MASK) != 0 { return -KVM_EINVAL; }
    let mode = icr as u32 & ICR_DM_MASK;
    let vector = (icr & 0xFF) as u8;
    let mut count = 0;
    for (word, base) in [(low, min), (high, min.wrapping_add(64))] {
        let mut bits = word;
        while bits != 0 {
            let i = bits.trailing_zeros() as u64;
            bits &= bits - 1;
            let dest = base.wrapping_add(i);
            if dest >= MAX_VCPUS as u64 { continue; }
            deliver_ipi(sender, &IcrWrite { delivery_mode: mode, shorthand: 0, dest: dest as u8, vector });
            count += 1;
        }
    }
    count
}

/// Deliver a message-signalled interrupt (I/O APIC redirection entry or MSI)
/// to its destination LAPIC(s) — `kvm_irq_delivery_to_apic`. `dest` is the
/// physical APIC ID (0xFF = broadcast) or, `logical`, a flat-model bitmask;
/// lowest priority goes to the first vCPU named.
pub fn deliver_msg(dest: u8, logical: bool, mode: u8, vector: u8, from: u8) {
    let delivery_mode = match mode {
        0 => ICR_DM_FIXED,
        1 => ICR_DM_LOWEST,
        _ => return, // SMI / NMI / INIT / ExtINT: no such source here
    };
    let send = |t: u8| deliver_ipi(from, &IcrWrite { delivery_mode, shorthand: 0, dest: t, vector });
    let n = crate::microvm::cpu::guest_vcpus();
    if !logical {
        if dest == 0xFF { (0..n).for_each(send); } else { send(dest); }
        return;
    }
    let mut targets = (0..n.min(8)).filter(|&i| dest & (1 << i) != 0);
    if delivery_mode == ICR_DM_LOWEST {
        if let Some(t) = targets.next() { send(t); }
    } else {
        targets.for_each(send);
    }
}

/// Clear every descriptor (VM start/teardown).
pub fn reset_posted() {
    for t in PIR.iter() {
        for w in t.iter() { w.store(0, Ordering::Relaxed); }
    }
    for on in PIR_ON.iter() { on.store(false, Ordering::Relaxed); }
    for c in VCPU_HOST_CORE.iter() { c.store(usize::MAX, Ordering::Relaxed); }
    for a in VCPU_LAST_ACTIVE.iter() { a.store(0, Ordering::Relaxed); }
}

pub struct LocalApic {
    /// The register page, indexed by `offset >> 4` (whole 4 KiB page, so any
    /// in-page offset is a safe store).
    regs: [u32; 256],
    apic_id: u8,
    /// Host TSC the current timer count started from.
    timer_start_tsc: u64,
    /// One-shot: fired once for the current TMICT write.
    timer_fired: bool,
    /// Periodic ticks owed (IRR already held the vector when they fell due).
    timer_owed: u32,
    /// IA32_APIC_BASE.EXTD: registers are MSRs 0x800+, ICR is one 64-bit write.
    x2apic: bool,
    /// IA32_TSC_DEADLINE in guest TSC = host TSC (no TSC offset); 0 = disarmed.
    tsc_deadline: u64,
    /// Guest address of its PV-EOI flag (`MSR_KVM_PV_EOI_EN`), 0 = off.
    pv_eoi_gpa: u64,
    /// `KVM_APIC_PV_EOI_PENDING`: we set the guest's flag before this entry.
    pv_eoi_pending: bool,
}

impl LocalApic {
    /// `kvm_lapic_reset`: LVTs masked, except the BSP's LVT0 in ExtINT
    /// (KVM_X86_QUIRK_LINT0_REENABLED) — the virtual wire the PIC uses.
    pub fn new(apic_id: u8) -> Self {
        let mut regs = [0u32; 256];
        regs[idx(APIC_LVR)] = LVR_VALUE;
        regs[idx(APIC_ID)] = (apic_id as u32) << 24;
        regs[idx(APIC_SPIV)] = 0xFF;
        for lvt in [APIC_LVTT, APIC_LVTTHMR, APIC_LVTPC, APIC_LVT0, APIC_LVT1, APIC_LVTERR] {
            regs[idx(lvt)] = LVT_MASKED;
        }
        if apic_id == 0 {
            regs[idx(APIC_LVT0)] = APIC_MODE_EXTINT;
        }
        LocalApic {
            regs, apic_id, timer_start_tsc: 0, timer_fired: false, timer_owed: 0,
            x2apic: false, tsc_deadline: 0, pv_eoi_gpa: 0, pv_eoi_pending: false,
        }
    }

    #[inline]
    fn sw_enabled(&self) -> bool { self.regs[idx(APIC_SPIV)] & SPIV_APIC_ENABLED != 0 }

    // ── IRR / ISR bitmaps (8 × 32-bit words at 16-byte stride) ──

    fn set_bit(&mut self, base: u32, v: u8) {
        self.regs[idx(base) + (v >> 5) as usize] |= 1 << (v & 31);
    }
    fn clear_bit(&mut self, base: u32, v: u8) {
        self.regs[idx(base) + (v >> 5) as usize] &= !(1 << (v & 31));
    }
    fn test_bit(&self, base: u32, v: u8) -> bool {
        self.regs[idx(base) + (v >> 5) as usize] & (1 << (v & 31)) != 0
    }
    /// `apic_search_irr` / `apic_find_highest_isr`.
    fn find_highest(&self, base: u32) -> Option<u8> {
        for w in (0..8).rev() {
            let word = self.regs[idx(base) + w];
            if word != 0 {
                return Some((w as u32 * 32 + 31 - word.leading_zeros()) as u8);
            }
        }
        None
    }

    /// `__apic_update_ppr`.
    fn update_ppr(&mut self) -> u32 {
        let tpr = self.regs[idx(APIC_TASKPRI)];
        let isrv = self.find_highest(APIC_ISR).unwrap_or(0) as u32;
        let ppr = if (tpr & 0xF0) >= (isrv & 0xF0) { tpr & 0xFF } else { isrv & 0xF0 };
        self.regs[idx(APIC_PROCPRI)] = ppr;
        ppr
    }

    /// `__apic_accept_irq` for a fixed, edge-triggered vector.
    pub fn accept_irq(&mut self, vector: u8) {
        if !self.sw_enabled() { return; }
        self.set_bit(APIC_IRR, vector);
        self.clear_bit(APIC_TMR, vector);
    }

    /// `sync_pir_to_irr`: fold posted vectors into IRR.
    fn sync_posted(&mut self) {
        let t = self.apic_id as usize;
        if t >= MAX_VCPUS { return; }
        let _ = PIR_ON[t].swap(false, Ordering::AcqRel);
        for w in 0..4 {
            let bits = PIR[t][w].swap(0, Ordering::AcqRel);
            if bits == 0 { continue; }
            for b in 0..64 {
                if bits & (1 << b) != 0 { self.accept_irq((w * 64 + b) as u8); }
            }
        }
    }

    /// `kvm_apic_has_interrupt`: highest IRR vector above PPR, after folding
    /// posted interrupts and an expired timer into IRR.
    pub fn has_interrupt(&mut self) -> Option<u8> {
        self.sync_posted();
        self.timer_poll();
        if !self.sw_enabled() { return None; }
        let ppr = self.update_ppr();
        let irr = self.find_highest(APIC_IRR)?;
        if (irr as u32 & 0xF0) <= ppr { return None; }
        Some(irr)
    }

    /// `kvm_apic_ack_interrupt`: IRR → ISR for `vector`.
    pub fn ack_interrupt(&mut self, vector: u8) {
        self.clear_bit(APIC_IRR, vector);
        self.set_bit(APIC_ISR, vector);
        self.update_ppr();
    }

    /// `kvm_apic_accept_pic_intr`: does this vCPU take the PIC's INTR (LINT0
    /// in ExtINT, the virtual wire)?
    pub fn accept_pic_intr(&self) -> bool {
        let lvt0 = self.regs[idx(APIC_LVT0)];
        lvt0 & LVT_MASKED == 0 && lvt0 & 0x700 == APIC_MODE_EXTINT
    }

    // ── Timer (KVM `start_sw_timer` / `apic_timer_expired`) ──

    /// TDCR → divide count (`update_divide_count`).
    fn divide_count(&self) -> u64 {
        let tdcr = self.regs[idx(APIC_TDCR)] & 0xF;
        let shift = ((tdcr & 0x3) | ((tdcr & 0x8) >> 1)) + 1;
        1u64 << (shift & 0x7)
    }

    /// Period of one count in host TSC cycles, if the timer is armed.
    fn period(&self) -> Option<u64> {
        if self.tscdeadline_mode() { return None; }
        let tmict = self.regs[idx(APIC_TMICT)];
        if !self.sw_enabled() || self.regs[idx(APIC_LVTT)] & LVT_MASKED != 0 || tmict == 0 {
            return None;
        }
        let p = tmict as u64 * self.divide_count();
        (p != 0).then_some(p)
    }

    fn periodic(&self) -> bool {
        self.regs[idx(APIC_LVTT)] & LVT_TIMER_MODE_MASK == LVT_TIMER_PERIODIC
    }

    fn tscdeadline_mode(&self) -> bool {
        self.regs[idx(APIC_LVTT)] & LVT_TIMER_MODE_MASK == LVT_TIMER_TSCDEADLINE
    }

    /// TSC-deadline expiry (`start_sw_tscdeadline` → `apic_timer_expired`):
    /// the deadline is consumed; a masked LVTT delivers nothing.
    fn tscdeadline_poll(&mut self) {
        if self.tsc_deadline == 0 || rdtsc() < self.tsc_deadline { return; }
        self.tsc_deadline = 0;
        let lvtt = self.regs[idx(APIC_LVTT)];
        if self.sw_enabled() && lvtt & LVT_MASKED == 0 {
            self.accept_irq((lvtt & VECTOR_MASK) as u8);
            crate::microvm::devices::nat::note_guest_timer();
        }
    }

    /// Expire the timer: set LVTT's vector in IRR. A periodic tick that falls
    /// due while the previous one still sits in IRR is owed (bounded) and
    /// posted once IRR is free again.
    fn timer_poll(&mut self) {
        if self.tscdeadline_mode() {
            self.tscdeadline_poll();
            return;
        }
        let Some(period) = self.period() else { return };
        let now = rdtsc();
        let elapsed = now.saturating_sub(self.timer_start_tsc);
        let periods = elapsed / period;
        if periods > 0 {
            self.timer_start_tsc = self.timer_start_tsc.wrapping_add(periods * period);
            if self.periodic() {
                self.timer_owed = (self.timer_owed + periods.min(MAX_PENDING as u64) as u32)
                    .min(MAX_PENDING);
            } else if !self.timer_fired {
                self.timer_fired = true;
                self.timer_owed = 1;
            }
        }
        if self.timer_owed > 0 {
            let vec = (self.regs[idx(APIC_LVTT)] & VECTOR_MASK) as u8;
            if !self.test_bit(APIC_IRR, vec) {
                self.accept_irq(vec);
                self.timer_owed -= 1;
                crate::microvm::devices::nat::note_guest_timer();
            }
        }
    }

    /// Host TSC of the next timer expiry (park and host one-shot), or None.
    pub fn next_timer_deadline_tsc(&self) -> Option<u64> {
        if self.tscdeadline_mode() {
            return (self.tsc_deadline != 0).then_some(self.tsc_deadline);
        }
        let period = self.period()?;
        // An owed tick is not a deadline: it is delivered on the exit the
        // guest's EOI of the previous one causes.
        if !self.periodic() && self.timer_fired { return None; }
        Some(self.timer_start_tsc.wrapping_add(period))
    }

    /// APIC_TMCCT (`apic_get_tmcct`).
    fn tmcct(&self) -> u32 {
        if self.tscdeadline_mode() { return 0; }
        let tmict = self.regs[idx(APIC_TMICT)];
        if tmict == 0 { return 0; }
        let elapsed_ticks = rdtsc().saturating_sub(self.timer_start_tsc) / self.divide_count();
        if self.periodic() {
            (tmict as u64 - (elapsed_ticks % tmict as u64)) as u32
        } else if elapsed_ticks >= tmict as u64 {
            0
        } else {
            (tmict as u64 - elapsed_ticks) as u32
        }
    }

    /// `apic_set_eoi`: clear the highest in-service vector.
    fn set_eoi(&mut self) {
        if let Some(v) = self.find_highest(APIC_ISR) {
            self.clear_bit(APIC_ISR, v);
            self.update_ppr();
        }
    }

    // ── PV EOI (KVM `apic_sync_pv_eoi_to_guest` / `_from_guest`) ──
    //
    // With the flag set, the guest's EOI is a bit clear in its own memory
    // instead of a trapped register write; we complete it at the next exit.
    // Offered only when the EOI can have no side effect: nothing else pending
    // in IRR and exactly one vector in service (edge-triggered — every vector
    // this LAPIC accepts is).

    /// Before entry: arm the guest's flag if its next EOI may be lazy.
    pub fn pv_eoi_sync_to(&mut self, mem: &crate::microvm::devices::guest_mem::GuestMem) {
        if self.pv_eoi_gpa == 0 || self.pv_eoi_pending { return; }
        if self.find_highest(APIC_IRR).is_some() || self.isr_count() != 1 { return; }
        if mem.write_u8(self.pv_eoi_gpa, 1) {
            self.pv_eoi_pending = true;
        }
    }

    /// After exit: flag cleared by the guest = it did EOI → complete it here;
    /// still set = no EOI yet → take the flag back (its EOI then traps).
    pub fn pv_eoi_sync_from(&mut self, mem: &crate::microvm::devices::guest_mem::GuestMem) {
        if !self.pv_eoi_pending { return; }
        self.pv_eoi_pending = false;
        match mem.read_u8(self.pv_eoi_gpa) {
            Some(v) if v & 1 != 0 => { let _ = mem.write_u8(self.pv_eoi_gpa, v & !1); }
            _ => self.set_eoi(),
        }
    }

    fn isr_count(&self) -> u32 {
        (0..8).map(|w| self.regs[idx(APIC_ISR) + w].count_ones()).sum()
    }

    // ── MSR interface: IA32_APIC_BASE, x2APIC, PV-EOI enable ──

    /// `Some(result)` if `msr` belongs to the LAPIC, `None` otherwise.
    /// A write may yield an IPI for the caller to route.
    pub fn msr(&mut self, msr: u32, write: Option<u64>) -> Option<Result<(u64, Option<IcrWrite>), ()>> {
        Some(match (msr, write) {
            (MSR_APIC_BASE, None) => {
                let mut v = APIC_BASE_MSR_VALUE;
                if self.apic_id != 0 { v &= !(1u64 << 8); }
                if self.x2apic { v |= APIC_BASE_X2APIC; }
                Ok((v, None))
            }
            (MSR_APIC_BASE, Some(v)) => {
                // xAPIC → x2APIC needs the global enable; x2APIC → xAPIC is
                // not a legal transition (SDM 10.12.5) and is ignored.
                if v & APIC_BASE_X2APIC != 0 && v & APIC_BASE_ENABLE != 0 && !self.x2apic {
                    self.x2apic = true;
                    self.regs[idx(APIC_ID)] = self.apic_id as u32;
                }
                Ok((0, None))
            }
            // `kvm_set_lapic_tscdeadline_msr`: only in TSC-deadline mode.
            (MSR_TSC_DEADLINE, None) => {
                Ok((if self.tscdeadline_mode() { self.tsc_deadline } else { 0 }, None))
            }
            (MSR_TSC_DEADLINE, Some(v)) => {
                if self.tscdeadline_mode() { self.tsc_deadline = v; }
                Ok((0, None))
            }
            (MSR_KVM_PV_EOI_EN, None) => {
                Ok((if self.pv_eoi_gpa != 0 { self.pv_eoi_gpa | KVM_MSR_ENABLED } else { 0 }, None))
            }
            (MSR_KVM_PV_EOI_EN, Some(v)) => {
                if v & 0x2 != 0 { return Some(Err(())); } // reserved, 4-byte aligned
                self.pv_eoi_gpa = if v & KVM_MSR_ENABLED != 0 { v & !0x3 } else { 0 };
                self.pv_eoi_pending = false;
                Ok((0, None))
            }
            (MSR_X2APIC_FIRST..=MSR_X2APIC_LAST, w) => {
                if !self.x2apic { return Some(Err(())); }
                self.x2apic_access((msr - MSR_X2APIC_FIRST) << 4, w)
            }
            _ => return None,
        })
    }

    /// `kvm_x2apic_msr_read` / `kvm_x2apic_msr_write`.
    fn x2apic_access(&mut self, off: u32, write: Option<u64>) -> Result<(u64, Option<IcrWrite>), ()> {
        match (off, write) {
            // No DFR, no ICR2 in x2APIC mode; SELF_IPI is write-only.
            (APIC_DFR, _) | (APIC_ICR2, _) | (X2APIC_SELF_IPI, None) => Err(()),
            (APIC_ICR, None) => Ok((
                self.regs[idx(APIC_ICR)] as u64 | (self.regs[idx(APIC_ICR2)] as u64) << 32,
                None,
            )),
            (APIC_LDR, None) => {
                // Cluster = id[31:4], bit id[3:0] (x2APIC logical ID, read-only).
                let id = self.apic_id as u32;
                Ok((((id >> 4) << 16 | 1 << (id & 0xF)) as u64, None))
            }
            (_, None) => Ok((self.read(off) as u64, None)),
            (APIC_ICR, Some(v)) => {
                note_access(APIC_ICR, true);
                let lo = v as u32;
                self.regs[idx(APIC_ICR)] = lo & !ICR_BUSY;
                self.regs[idx(APIC_ICR2)] = (v >> 32) as u32;
                Ok((0, Some(IcrWrite {
                    delivery_mode: lo & ICR_DM_MASK,
                    shorthand: (lo >> 18) & 0x3,
                    dest: (v >> 32) as u8,
                    vector: (lo & VECTOR_MASK) as u8,
                })))
            }
            (X2APIC_SELF_IPI, Some(v)) => Ok((0, Some(IcrWrite {
                delivery_mode: ICR_DM_FIXED,
                shorthand: 1,
                dest: self.apic_id,
                vector: (v & 0xFF) as u8,
            }))),
            // Read-only in x2APIC mode.
            (APIC_ID | APIC_LDR, Some(_)) => Err(()),
            (APIC_EOI, Some(v)) if v != 0 => Err(()),
            (_, Some(v)) => {
                if v >> 32 != 0 { return Err(()); }
                let _ = self.write(off, v as u32);
                Ok((0, None))
            }
        }
    }

    // ── MMIO (`kvm_lapic_reg_read` / `kvm_lapic_reg_write`) ──

    pub fn read(&mut self, off: u32) -> u32 {
        note_access(off, false);
        match off & 0xFF0 {
            APIC_TMCCT => self.tmcct(),
            APIC_PROCPRI => self.update_ppr(),
            o => self.regs[idx(o)],
        }
    }

    /// Returns the decoded IPI for an ICR-low write; the backend routes it.
    #[must_use]
    pub fn write(&mut self, off: u32, val: u32) -> Option<IcrWrite> {
        note_access(off, true);
        match off & 0xFF0 {
            APIC_EOI => self.set_eoi(),
            APIC_ID => self.regs[idx(APIC_ID)] = val,
            APIC_TASKPRI => {
                self.regs[idx(APIC_TASKPRI)] = val & 0xFF;
                self.update_ppr();
            }
            APIC_SPIV => {
                self.regs[idx(APIC_SPIV)] = val & 0x3FF;
                // Software-disabling masks every LVT (KVM `kvm_lapic_reg_write`).
                if val & SPIV_APIC_ENABLED == 0 {
                    for lvt in [APIC_LVTT, APIC_LVTTHMR, APIC_LVTPC, APIC_LVT0, APIC_LVT1, APIC_LVTERR] {
                        self.regs[idx(lvt)] |= LVT_MASKED;
                    }
                }
            }
            APIC_ESR => self.regs[idx(APIC_ESR)] = 0,
            // Ignored in TSC-deadline mode (KVM `kvm_lapic_reg_write`).
            APIC_TMICT if self.tscdeadline_mode() => {}
            APIC_TMICT => {
                self.regs[idx(APIC_TMICT)] = val;
                self.timer_start_tsc = rdtsc();
                self.timer_fired = false;
                self.timer_owed = 0;
            }
            APIC_TDCR => self.regs[idx(APIC_TDCR)] = val & 0xB,
            APIC_LVTT | APIC_LVTTHMR | APIC_LVTPC | APIC_LVT0 | APIC_LVT1 | APIC_LVTERR => {
                let mut v = val;
                if !self.sw_enabled() { v |= LVT_MASKED; }
                // A timer-mode change stops the timer (`apic_update_lvtt`).
                if off & 0xFF0 == APIC_LVTT
                    && (self.regs[idx(APIC_LVTT)] ^ v) & LVT_TIMER_MODE_MASK != 0
                {
                    self.regs[idx(APIC_TMICT)] = 0;
                    self.tsc_deadline = 0;
                    self.timer_owed = 0;
                    self.timer_fired = false;
                }
                self.regs[idx(off & 0xFF0)] = v;
            }
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
            // Read-only: LVR, PPR, ISR, TMR, IRR.
            APIC_LVR | APIC_PROCPRI => {}
            o if (APIC_ISR..APIC_ESR).contains(&o) => {}
            o => self.regs[idx(o)] = val,
        }
        None
    }
}
