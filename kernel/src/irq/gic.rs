//! The AArch64 half of device interrupts: SPIs of the GIC, after Linux'
//! GIC domain (`irq-gic.c`) and the generic flow handlers.
//!
//! An interrupt is an SPI the device raises itself (`register_spi`) or a
//! line of a controller in front of the GIC that turns its lines into SPIs
//! (`register_chained`), as RP1 does through MSI-X and the MIP. Each runs
//! with `IRQF_ONESHOT` semantics: the hardirq masks it where Linux'
//! `handle_fasteoi_irq` or, for a chained line, `handle_level_irq` would,
//! and `arm` unmasks it once the driver's fiber has serviced the device.
//! `route_to_current` is `gic_set_affinity` towards the core the fiber
//! waits on.

use alloc::collections::BTreeMap;
use core::sync::atomic::{AtomicPtr, AtomicU16, AtomicU32, Ordering};
use spin::Mutex;

use super::{desc, note_fired, Desc, Irq};
use crate::arch::aarch64::gic;

/// One interrupt number per INTID at most.
pub const NR_IRQS: usize = gic::MAX_IRQS;

/// A controller between devices and the GIC whose lines arrive as SPIs
/// (Linux: an irq chip whose parent interrupt runs a chained handler).
pub trait Chained: Sync {
    /// Keep `line` from sending (`irq_mask` of the chip).
    fn mask(&self, line: u32);
    fn unmask(&self, line: u32);
    /// The chained handler's work after the line's handler ran, e.g.
    /// acknowledging a level line so it can send again.
    fn handled(&self, line: u32);
}

/// INTID → interrupt number, 0 = none: the GIC domain's mapping.
static INTID_IRQ: [AtomicU16; gic::MAX_IRQS] = [const { AtomicU16::new(0) }; gic::MAX_IRQS];
/// The chained controller in front of an INTID, null for a plain SPI. The
/// controllers are statics; this points at a `&'static dyn Chained`.
static CHAINED: [AtomicPtr<&'static dyn Chained>; gic::MAX_IRQS] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; gic::MAX_IRQS];
/// The controller's line behind an INTID.
static CHAINED_LINE: [AtomicU32; gic::MAX_IRQS] = [const { AtomicU32::new(0) }; gic::MAX_IRQS];

/// Where an interrupt is delivered now.
#[derive(Clone, Copy)]
struct Reg {
    intid: u32,
    core: usize,
}

static REGS: Mutex<BTreeMap<Irq, Reg>> = Mutex::new(BTreeMap::new());

fn chained(intid: u32) -> Option<(&'static dyn Chained, u32)> {
    let p = CHAINED.get(intid as usize)?.load(Ordering::Acquire);
    // SAFETY: only `register_chained` stores here, a pointer to a
    // `&'static dyn Chained` that lives as long as the kernel.
    (!p.is_null()).then(|| (unsafe { *p }, CHAINED_LINE[intid as usize].load(Ordering::Acquire)))
}

/// An interrupt number for INTID `intid`, delivered to the current core
/// and masked until the first `arm`.
fn bind(intid: u32) -> Option<(Irq, &'static Desc)> {
    if intid < gic::SPI_BASE || intid as usize >= gic::MAX_IRQS {
        return None;
    }
    if INTID_IRQ[intid as usize].load(Ordering::Acquire) != 0 {
        crate::kprintln!("[npk] irq: INTID {} is taken", intid);
        return None;
    }
    let irq = super::alloc_irq()?;
    let d = desc(irq)?;
    d.hwirq.store(intid, Ordering::Relaxed);
    d.oneshot.store(true, Ordering::Release);
    let core = crate::smp::per_core::current_core_id();
    gic::set_affinity(intid, core);
    REGS.lock().insert(irq, Reg { intid, core });
    INTID_IRQ[intid as usize].store(irq, Ordering::Release);
    Some((irq, d))
}

/// An SPI the device raises itself: edge-rising or level-high
/// (`gic_set_type`). Masked until the driver `arm`s it.
pub fn register_spi(intid: u32, edge: bool) -> Option<Irq> {
    gic::mask(intid);
    if !gic::set_type(intid, edge) {
        crate::kprintln!("[npk] irq: INTID {} does not take its trigger type", intid);
        return None;
    }
    let (irq, _) = bind(intid)?;
    gic::set_handler(intid, spi_isr);
    Some(irq)
}

/// Line `line` of `chip`, which arrives as the edge-triggered SPI `intid`.
/// The SPI stays enabled, as Linux starts a chained handler's parent at
/// once; the line is masked at `chip` until the driver `arm`s it.
pub fn register_chained(chip: &'static &'static dyn Chained, line: u32, intid: u32) -> Option<Irq> {
    chip.mask(line);
    gic::mask(intid);
    if !gic::set_type(intid, true) {
        crate::kprintln!("[npk] irq: INTID {} does not take an edge trigger", intid);
        return None;
    }
    let (irq, _) = bind(intid)?;
    CHAINED_LINE[intid as usize].store(line, Ordering::Relaxed);
    CHAINED[intid as usize].store(chip as *const &'static dyn Chained as *mut _, Ordering::Release);
    gic::set_handler(intid, spi_isr);
    gic::unmask(intid);
    Some(irq)
}

/// The hardirq of every registered SPI, between the GIC's priority drop
/// and its deactivation (`gic::handle`).
fn spi_isr(intid: u32) {
    let irq = INTID_IRQ.get(intid as usize).map_or(0, |a| a.load(Ordering::Acquire));
    let Some(d) = desc(irq) else { return };
    match chained(intid) {
        // rp1_chained_handle_irq → handle_level_irq: mask, handle, then the
        // controller's own acknowledge.
        Some((chip, line)) => {
            chip.mask(line);
            note_fired(d);
            chip.handled(line);
        }
        // handle_fasteoi_irq with IRQF_ONESHOT.
        None => {
            gic::mask(intid);
            note_fired(d);
        }
    }
}

/// Deliver `irq` to the current core (`gic_set_affinity`; for a chained
/// line Linux sets the parent SPI's affinity the same way). No-op if `irq`
/// is not registered or already targets this core.
pub fn route_to_current(irq: Irq) {
    let core = crate::smp::per_core::current_core_id();
    let mut regs = REGS.lock();
    let Some(r) = regs.get_mut(&irq) else { return };
    if r.core == core {
        return;
    }
    if gic::set_affinity(r.intid, core) {
        r.core = core;
    } else {
        crate::kprintln!("[npk] irq: core {} has no GIC CPU interface; irq {} stays on core {}",
            core, irq, r.core);
    }
}

/// The driver has serviced the device: let the interrupt through again.
pub fn unmask(_irq: Irq, d: &Desc) {
    let intid = d.hwirq.load(Ordering::Relaxed);
    match chained(intid) {
        Some((chip, line)) => chip.unmask(line),
        None => gic::unmask(intid),
    }
}

/// The routing half of `irq::release`: the line masked, the INTID free.
/// False if `irq` was not registered here.
pub fn release_source(irq: Irq, d: &Desc) -> bool {
    let Some(r) = REGS.lock().remove(&irq) else { return false };
    let intid = d.hwirq.load(Ordering::Relaxed);
    if let Some((chip, line)) = chained(intid) {
        chip.mask(line);
    }
    gic::mask(r.intid);
    gic::clear_handler(r.intid);
    CHAINED[r.intid as usize].store(core::ptr::null_mut(), Ordering::Release);
    INTID_IRQ[r.intid as usize].store(0, Ordering::Release);
    true
}
