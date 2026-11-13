//! The AArch64 half of SMP: cores from the device tree, started by PSCI
//! (`arch::aarch64::cores`), told apart by their index in TPIDR, woken by
//! an SGI through the GIC and halted with WFI until the generic timer's
//! one-shot or another interrupt ends the wait.

use super::{per_core, scheduler};
use crate::arch::aarch64::{cores, cpu, gic, timer};
use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicBool, Ordering};

/// Core 0 runs on the one-shot timer too (`make_core0_tickless`).
static CORE0_TICKLESS: AtomicBool = AtomicBool::new(false);

/// Start the other cores and the scheduler; returns the cores running,
/// the boot core included.
pub fn init(fdt: &Fdt) -> usize {
    gic::enable(gic::SGI_WAKE, wake_handler);
    let n = cores::start(fdt, ap_entry);
    per_core::set_core_count(n);
    if n > 1 {
        scheduler::init(n - 1);
        per_core::start_scheduler();
    }
    n
}

/// A started core: its GIC interface and timer are up, interrupts masked.
fn ap_entry(index: usize) -> ! {
    gic::unmask(gic::SGI_WAKE);
    per_core::worker_loop(index as u32)
}

/// The wake SGI: taking it is what ends the halt.
fn wake_handler(_irq: u32) {
    if CORE0_TICKLESS.load(Ordering::Relaxed) && cpu::current_core() == 0 {
        per_core::record_wake(0, per_core::WAKE_TIMER);
    }
}

fn timer_fired() {
    let cid = cpu::current_core();
    per_core::note_timer_fire(cid);
    if CORE0_TICKLESS.load(Ordering::Relaxed) && cid == 0 {
        per_core::record_wake(0, per_core::WAKE_TIMER);
    }
}

pub fn current_core_id() -> usize {
    cpu::current_core()
}

/// The current core's id; the boot code sets it before anything runs.
pub fn ready_core_id() -> Option<usize> {
    Some(current_core_id())
}

/// The wake SGI to core `c`.
pub fn send_wake(c: usize) {
    gic::send_sgi(c, gic::SGI_WAKE);
}

/// The calling core's timer becomes the one-shot its idle halt arms.
pub fn init_worker_timer() {
    timer::make_oneshot(timer_fired);
}

/// Core 0's periodic tick ends; from here it wakes only when something is
/// due or something happens.
pub fn make_core0_tickless() {
    init_worker_timer();
    CORE0_TICKLESS.store(true, Ordering::Release);
}

/// Halt until an interrupt, and no later than counter value `deadline`
/// if one is given, with the accounting of x86's `interrupts::halt_until`.
/// The one-shot is armed with interrupts masked and the halt is a WFI that
/// a pending interrupt ends: one that lands between the arm and the halt
/// is not lost. Core 0 before it went tickless keeps its tick.
pub fn halt_until(deadline: Option<u64>, cause: usize) {
    let cid = cpu::current_core();
    let own_timer = cid < cores::MAX_CORES && (cid != 0 || CORE0_TICKLESS.load(Ordering::Relaxed));
    let irq = crate::arch::irq_save();
    if let Some(d) = deadline {
        if crate::time::counter() >= d {
            crate::arch::irq_restore(irq);
            return;
        }
        if own_timer { timer::arm_at(d); }
    }
    let t0 = crate::time::counter();
    per_core::halt_begin(cid, t0);
    cpu::wait_for_interrupt();
    per_core::note_woke(cid);
    let t1 = crate::time::counter();
    per_core::record_halt(cid, t1.saturating_sub(t0));
    if let Some(d) = deadline {
        per_core::note_deadline_late(cid, t1.saturating_sub(d));
    }
    // Core 0's wakes are attributed by its handlers.
    if cid != 0 {
        per_core::record_wake(cid, cause);
    }
    // Woken by something else: the one-shot is still pending.
    if own_timer && deadline.is_some() { timer::disarm(); }
    crate::arch::irq_restore(irq);
}

// The WASM NIC driver and the microVM are not ported: no core holds them.

pub fn nic_core() -> Option<usize> {
    None
}

pub fn is_vcpu_core(_c: usize) -> bool {
    false
}

pub fn in_guest(_c: usize) -> bool {
    false
}

pub fn vm_core_serve() {}

/// Arm cores have no APERF/MPERF.
pub fn sample_aperf_mperf(_core_id: usize) {}

pub fn aperf_mperf_of(_core_id: usize) -> (u64, u64) {
    (0, 0)
}
