//! The generic timer's interrupt, after Linux
//! `drivers/clocksource/arm_arch_timer.c`: which timer and PPI a kernel
//! uses (`arch_timer_select_ppi`), arming it by compare value and the
//! handler that masks it when it fires.
//!
//! With VHE the kernel owns EL2 and the CNTP_* names reach the EL2
//! physical timer (CNTHP), whose interrupt is the device tree's hyp PPI;
//! at EL1 they reach the EL1 physical timer and its non-secure PPI.

use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

const ARCH_TIMER_CTRL_ENABLE: u64 = 1 << 0;
const ARCH_TIMER_CTRL_IT_MASK: u64 = 1 << 1;
const ARCH_TIMER_CTRL_IT_STAT: u64 = 1 << 2;

/// `interrupts` order of an "arm,armv8-timer" node.
const ARCH_TIMER_PHYS_NONSECURE_PPI: usize = 1;
const ARCH_TIMER_HYP_PPI: usize = 3;

/// Tick rate: the 10 ms of `time::ticks`.
pub const HZ: u64 = 100;

static PERIOD: AtomicU64 = AtomicU64::new(0);
/// The timer's INTID, a PPI: one line, banked per core.
static IRQ: AtomicU32 = AtomicU32::new(0);
static FIRED: AtomicU64 = AtomicU64::new(0);

fn ctrl() -> u64 {
    let v: u64;
    // SAFETY: reads this core's timer control, no other effect.
    unsafe { core::arch::asm!("mrs {}, CNTP_CTL_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

fn set_ctrl(v: u64) {
    // SAFETY: writes this core's timer control; nothing else depends on it.
    unsafe { core::arch::asm!("msr CNTP_CTL_EL0, {}", "isb", in(reg) v, options(nostack, preserves_flags)) };
}

fn physical_count() -> u64 {
    let v: u64;
    // SAFETY: reads the physical count, which the compare value is against.
    unsafe { core::arch::asm!("isb", "mrs {}, CNTPCT_EL0", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

/// arch_timer_set_next_event: fire `delta` counter units from now.
fn set_next_event(delta: u64) {
    let c = (ctrl() | ARCH_TIMER_CTRL_ENABLE) & !ARCH_TIMER_CTRL_IT_MASK;
    let cval = physical_count() + delta;
    // SAFETY: writes this core's compare value.
    unsafe { core::arch::asm!("msr CNTP_CVAL_EL0, {}", in(reg) cval, options(nostack, preserves_flags)) };
    set_ctrl(c);
}

/// timer_handler, with the periodic tick as its event handler.
fn handler() {
    let c = ctrl();
    if c & ARCH_TIMER_CTRL_IT_STAT == 0 { return; }
    set_ctrl(c | ARCH_TIMER_CTRL_IT_MASK);
    FIRED.fetch_add(1, Ordering::Relaxed);
    set_next_event(PERIOD.load(Ordering::Relaxed));
}

/// Timer interrupts taken since boot, on every core.
pub fn fired() -> u64 {
    FIRED.load(Ordering::Relaxed)
}

/// The INTID of the timer this kernel uses, from the device tree.
fn ppi(fdt: &Fdt) -> Option<u32> {
    let node = fdt.find_compatible(b"arm,armv8-timer")?;
    let (o, len) = fdt.prop(&node, b"interrupts")?;
    let which = if super::exception_level() == 2 { ARCH_TIMER_HYP_PPI } else { ARCH_TIMER_PHYS_NONSECURE_PPI };
    if len < 12 * (which + 1) { return None; }
    let number = fdt.cell_u32(o + 12 * which + 4);
    Some(super::gic::PPI_BASE + number)
}

/// Start the periodic tick on the calling core. The GIC must be up.
pub fn start(fdt: &Fdt) -> Option<u32> {
    let irq = ppi(fdt)?;
    PERIOD.store((super::time::counter_hz() / HZ).max(1), Ordering::Relaxed);
    IRQ.store(irq, Ordering::Relaxed);
    set_ctrl(ARCH_TIMER_CTRL_IT_MASK);
    super::gic::enable(irq, handler);
    set_next_event(PERIOD.load(Ordering::Relaxed));
    Some(irq)
}

/// The same tick on a started core, after the boot core's `start`.
pub fn start_this_core() {
    let irq = IRQ.load(Ordering::Relaxed);
    if irq == 0 { return; }
    set_ctrl(ARCH_TIMER_CTRL_IT_MASK);
    super::gic::unmask(irq);
    set_next_event(PERIOD.load(Ordering::Relaxed));
}

/// Entered from `irq_entry` in vectors.s with the interrupted context saved.
#[unsafe(no_mangle)]
extern "C" fn aarch64_irq(_frame: *mut u64) {
    super::gic::handle();
}
