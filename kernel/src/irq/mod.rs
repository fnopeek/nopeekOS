//! Device interrupts: a number per interrupt, the count its hardirq bumps
//! and the fiber it wakes. Where an interrupt comes from and how it reaches
//! a core is the architecture's half, `hw`: MSI-X, plain MSI or an I/O APIC
//! line through the LAPIC on x86-64, an SPI of the GIC on AArch64.
//!
//! The hardirq does the minimum: mask a line that would fire again before
//! it is serviced, bump the interrupt's fired count, signal the waiting
//! fiber. The driver services the device in that fiber, as Linux runs a
//! threaded handler (`request_threaded_irq` with `IRQF_ONESHOT`). The
//! interrupt follows the fiber: `arm` points it at the core the fiber waits
//! on, so the interrupt itself ends that core's halt and no core is set
//! apart for interrupts.
//!
//! ## Driver contract (host + WASM)
//!
//! Run the driver as a fiber. Once, after binding the device, register the
//! interrupt (`hw`: `register` for MSI/MSI-X, `register_gsi`, or
//! `register_spi`/`register_chained` on the GIC). Then loop, servicing on
//! the same fiber:
//! ```text
//!   loop {
//!       let since = irq::arm(irq);         // snapshot + route the IRQ to this core
//!       // enable / submit the device work that will raise the IRQ
//!       irq::wait(irq, since, timeout_ms); // park until it fires (or timeout)
//!       // service (drain the ring / read the completion)
//!   }
//! ```
//! Rules:
//! - `arm()` before the device submit closes the lost-wakeup window (an IRQ that
//!   races the park still advances the count past the snapshot).
//! - `arm()` re-routes the interrupt to the calling core, so the driver may run
//!   on any core / migrate; the IRQ always wakes the core that's about to wait.
//! - Never hold a lock across `wait()`: a same-core fiber spinning on it
//!   would deadlock the parked waiter. Snapshot/submit under the lock, drop it,
//!   then `arm`/`wait`, then re-acquire to service.
//!
//! WASM drivers use the mirror host-fns `npk_irq_register` / `npk_irq_arm` /
//! `npk_irq_wait` (see `wasm.rs`).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

use crate::smp::fiber::{Waker, NO_WAKER};

#[cfg_attr(target_arch = "x86_64", path = "x86.rs")]
#[cfg_attr(target_arch = "aarch64", path = "gic.rs")]
mod hw;

pub use hw::route_to_current;
#[cfg(target_arch = "x86_64")]
pub use hw::{isr, register, register_gsi};
#[cfg(target_arch = "aarch64")]
pub use hw::{register_chained, register_spi, Chained};

/// An interrupt number: what a register function hands out and a driver
/// passes back, 0 for none. It stays the same while the interrupt moves
/// from core to core, as Linux' IRQ number.
pub type Irq = u16;

/// Descriptors are allocated this many at a time, on first use.
const CHUNK: usize = 128;

/// The part of an interrupt the hardirq and a waiting fiber read without a
/// lock.
struct Desc {
    /// Bumped by the hardirq. A driver snapshots it before submitting work
    /// and waits for it to advance.
    fired: AtomicU64,
    /// The fiber parked on this interrupt; the hardirq signals it.
    waiter: AtomicU32,
    /// The interrupt's number at its controller: the I/O APIC input on
    /// x86-64, the INTID on the GIC.
    hwirq: AtomicU32,
    /// The hardirq masks the interrupt and `arm` unmasks it, as Linux'
    /// `IRQF_ONESHOT`: the line stays quiet until the driver has serviced
    /// the device, or a level line would fire again the moment the
    /// hardirq returns.
    oneshot: AtomicBool,
}

impl Desc {
    const fn new() -> Desc {
        Desc {
            fired: AtomicU64::new(0),
            waiter: AtomicU32::new(NO_WAKER),
            hwirq: AtomicU32::new(0),
            oneshot: AtomicBool::new(false),
        }
    }
}

/// Descriptor chunks, allocated when an interrupt number in their range is
/// first handed out and kept: numbers are reused before new ones are taken.
static DESCS: [AtomicPtr<[Desc; CHUNK]>; hw::NR_IRQS.div_ceil(CHUNK)] =
    [const { AtomicPtr::new(core::ptr::null_mut()) }; hw::NR_IRQS.div_ceil(CHUNK)];

fn desc(irq: Irq) -> Option<&'static Desc> {
    let i = (irq as usize).checked_sub(1)?;
    let chunk = DESCS.get(i / CHUNK)?.load(Ordering::Acquire);
    // SAFETY: a non-null chunk was leaked by `alloc_irq` and is never
    // freed.
    (!chunk.is_null()).then(|| unsafe { &(*chunk)[i % CHUNK] })
}

/// Numbers given back, taken before a new one, and the highest one handed
/// out so far.
struct Numbers {
    free: Vec<Irq>,
    top: Irq,
}

static NUMBERS: Mutex<Numbers> = Mutex::new(Numbers { free: Vec::new(), top: 0 });

/// A fresh interrupt number with a cleared descriptor.
fn alloc_irq() -> Option<Irq> {
    let irq = {
        let mut n = NUMBERS.lock();
        match n.free.pop() {
            Some(irq) => irq,
            None => {
                if n.top as usize >= hw::NR_IRQS {
                    crate::kprintln!("[npk] irq: all {} interrupt numbers in use", hw::NR_IRQS);
                    return None;
                }
                n.top += 1;
                n.top
            }
        }
    };
    let slot = &DESCS[(irq as usize - 1) / CHUNK];
    if slot.load(Ordering::Acquire).is_null() {
        let chunk = Box::leak(Box::new([const { Desc::new() }; CHUNK]));
        if slot.compare_exchange(core::ptr::null_mut(), chunk, Ordering::AcqRel, Ordering::Acquire).is_err() {
            // SAFETY: `chunk` was leaked just above and never published.
            drop(unsafe { Box::from_raw(chunk) });
        }
    }
    let d = desc(irq)?;
    d.waiter.store(NO_WAKER, Ordering::Release);
    d.oneshot.store(false, Ordering::Release);
    d.hwirq.store(0, Ordering::Relaxed);
    Some(irq)
}

fn free_irq(irq: Irq) {
    NUMBERS.lock().free.push(irq);
}

/// The hardirq's part once the line is quiet: count the interrupt, tell
/// the waiter. Release, so a parked fiber that sees the count advance also
/// sees the device data the interrupt signalled.
#[inline]
fn note_fired(d: &Desc) {
    d.fired.fetch_add(1, Ordering::Release);
    let w = d.waiter.load(Ordering::Acquire);
    if w != NO_WAKER {
        crate::smp::fiber::signal(w, crate::smp::fiber::SIG_IRQ);
    }
}

/// Register `w` as the fiber waiting on `irq`.
pub fn set_waiter(irq: Irq, w: Waker) {
    if let Some(d) = desc(irq) {
        d.waiter.store(w, Ordering::Release);
    }
}

/// Current fired count of `irq`. Acquire pairs with `note_fired`.
#[inline]
pub fn fired_count(irq: Irq) -> u64 {
    desc(irq).map_or(0, |d| d.fired.load(Ordering::Acquire))
}

/// Give `irq` back: no waiter, the interrupt masked at its controller and
/// its routing freed. The device must already be quiet (bus mastering off,
/// which also stops its MSI writes).
pub fn release(irq: Irq) {
    let Some(d) = desc(irq) else { return };
    d.waiter.store(NO_WAKER, Ordering::Release);
    if !hw::release_source(irq, d) {
        return;
    }
    d.oneshot.store(false, Ordering::Release);
    free_irq(irq);
}

/// Snapshot the fired count for `irq` before submitting the device
/// command (ringing the doorbell). Pass the returned token to `wait`. This
/// closes the lost-wakeup window: an IRQ that fires between submit and park
/// still advances the count past the snapshot, so `wait` returns at once.
///
/// Also routes the IRQ to the current core (where the caller will `wait`), so
/// the device's interrupt wakes this core out of its halt and its scheduler
/// resumes the parked fiber. Reprograms the routing only when the waiting
/// core changed: a no-op for a driver that always services on its own
/// pinned fiber. Last, a oneshot interrupt is unmasked: the driver has
/// serviced the device, so the next one may come.
pub fn arm(irq: Irq) -> u64 {
    let cid = crate::smp::per_core::current_core_id();
    if !crate::smp::per_core::is_vcpu_core(cid) {
        crate::smp::per_core::mark_driver_core(cid);
    }
    // The interrupt now targets this core; its waiter stays here.
    crate::smp::fiber::pin_current();
    route_to_current(irq);
    let since = fired_count(irq);
    if let Some(d) = desc(irq) {
        if d.oneshot.load(Ordering::Acquire) {
            hw::unmask(irq, d);
        }
    }
    since
}

/// Park the current fiber until `irq` fires (its count moves past `since`)
/// or `timeout_ms` elapses. Returns true if the IRQ fired, false on timeout.
/// Must be called from inside a fiber (returns false otherwise; the caller
/// should fall back to polling).
pub fn wait(irq: Irq, since: u64, timeout_ms: u64) -> bool {
    crate::smp::fiber::irq_wait(irq, since, timeout_ms)
}

/// `wait` without a timeout, as a Linux interrupt thread waits.
pub fn wait_event(irq: Irq, since: u64) -> bool {
    crate::smp::fiber::irq_wait_until(irq, since, crate::smp::fiber::NO_DEADLINE)
}
