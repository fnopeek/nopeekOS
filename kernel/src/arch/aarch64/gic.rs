//! The Arm GIC-400 (GICv2), after Linux `drivers/irqchip/irq-gic.c` and
//! `irq-gic-common.c`: the interrupt controller of the Raspberry Pi 5 and
//! of QEMU `virt` with `gic-version=2`.
//!
//! As Linux on a kernel that owns EL2 with an 8 KiB CPU interface, the
//! CPU interface runs with EOImodeNS: GICC_EOI drops the priority right
//! after the acknowledge, GICC_DIR deactivates once the handler is done.

use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

const GIC_CPU_CTRL: u64 = 0x00;
const GIC_CPU_PRIMASK: u64 = 0x04;
const GIC_CPU_INTACK: u64 = 0x0c;
const GIC_CPU_EOI: u64 = 0x10;
const GIC_CPU_ACTIVEPRIO: u64 = 0xd0;
const GIC_CPU_IDENT: u64 = 0xfc;
const GIC_CPU_DEACTIVATE: u64 = 0x1000;
const GICC_ENABLE: u32 = 0x1;
const GICC_INT_PRI_THRESHOLD: u32 = 0xf0;
const GIC_CPU_CTRL_EOIMODENS: u32 = 1 << 9;
const GICC_IAR_INT_ID_MASK: u32 = 0x3ff;
const GICC_DIS_BYPASS_MASK: u32 = 0x1e0;

const GIC_DIST_CTRL: u64 = 0x000;
const GIC_DIST_CTR: u64 = 0x004;
const GIC_DIST_ENABLE_SET: u64 = 0x100;
const GIC_DIST_ENABLE_CLEAR: u64 = 0x180;
const GIC_DIST_ACTIVE_CLEAR: u64 = 0x380;
const GIC_DIST_PRI: u64 = 0x400;
const GIC_DIST_TARGET: u64 = 0x800;
const GIC_DIST_CONFIG: u64 = 0xc00;
const GICD_ENABLE: u32 = 0x1;
const GICD_DISABLE: u32 = 0x0;
const GICD_INT_ACTLOW_LVLTRIG: u32 = 0x0;
const GICD_INT_EN_CLR_X32: u32 = 0xffff_ffff;
const GICD_INT_DEF_PRI: u8 = 0xa0;

/// INTIDs of the GIC: SGIs 0-15, PPIs 16-31, SPIs from 32, at most 1020.
pub const MAX_IRQS: usize = 1020;
/// The first PPI's INTID; device tree PPI numbers count from here.
pub const PPI_BASE: u32 = 16;
/// The first SPI's INTID; device tree SPI numbers count from here.
pub const SPI_BASE: u32 = 32;

static DIST: AtomicU64 = AtomicU64::new(0);
static CPU: AtomicU64 = AtomicU64::new(0);
static IRQS: AtomicU32 = AtomicU32::new(0);
/// EOImodeNS: priority drop and deactivation are separate writes.
static SPLIT_EOI: AtomicBool = AtomicBool::new(false);

/// Handler per INTID (a `fn()` as usize; 0 = none).
static HANDLERS: [AtomicUsize; MAX_IRQS] = [const { AtomicUsize::new(0) }; MAX_IRQS];

fn rd(base: &AtomicU64, off: u64) -> u32 {
    // SAFETY: `init` stored the GIC's register blocks from the device tree;
    // the identity map covers them as Device memory.
    unsafe { core::ptr::read_volatile((base.load(Ordering::Relaxed) + off) as *const u32) }
}

fn wr(base: &AtomicU64, off: u64, v: u32) {
    // SAFETY: as in `rd`.
    unsafe { core::ptr::write_volatile((base.load(Ordering::Relaxed) + off) as *mut u32, v) }
}

fn repeat_byte(b: u8) -> u32 {
    u32::from_ne_bytes([b; 4])
}

/// gic_check_gicv2: GICC_IIDR names an Arm GICv2.
fn is_gicv2() -> bool {
    let v = rd(&CPU, GIC_CPU_IDENT);
    (v & 0xff0fff) == 0x02043B
}

/// gic_get_cpumask: this core's bit in the target registers of the SGIs.
fn cpumask() -> u32 {
    let mut mask = 0;
    let mut i = 0;
    while i < 32 {
        mask = rd(&DIST, GIC_DIST_TARGET + i);
        mask |= mask >> 16;
        mask |= mask >> 8;
        if mask != 0 { break; }
        i += 4;
    }
    mask & 0xff
}

/// gic_dist_config.
fn dist_config(irqs: u32, pri: u8) {
    let mut i = 32;
    while i < irqs {
        wr(&DIST, GIC_DIST_CONFIG + (i / 4) as u64, GICD_INT_ACTLOW_LVLTRIG);
        i += 16;
    }
    let mut i = 32;
    while i < irqs {
        wr(&DIST, GIC_DIST_PRI + i as u64, repeat_byte(pri));
        i += 4;
    }
    let mut i = 32;
    while i < irqs {
        wr(&DIST, GIC_DIST_ACTIVE_CLEAR + (i / 8) as u64, GICD_INT_EN_CLR_X32);
        wr(&DIST, GIC_DIST_ENABLE_CLEAR + (i / 8) as u64, GICD_INT_EN_CLR_X32);
        i += 32;
    }
}

/// gic_dist_init.
fn dist_init() {
    let irqs = IRQS.load(Ordering::Relaxed);
    wr(&DIST, GIC_DIST_CTRL, GICD_DISABLE);
    let mut mask = cpumask();
    mask |= mask << 8;
    mask |= mask << 16;
    let mut i = 32;
    while i < irqs {
        wr(&DIST, GIC_DIST_TARGET + (i * 4 / 4) as u64, mask);
        i += 4;
    }
    dist_config(irqs, GICD_INT_DEF_PRI);
    wr(&DIST, GIC_DIST_CTRL, GICD_ENABLE);
}

/// gic_cpu_config + gic_cpu_init + gic_cpu_if_up, for the calling core.
pub fn cpu_init() {
    let mut i = 0;
    while i < 32 {
        wr(&DIST, GIC_DIST_ACTIVE_CLEAR + (i / 8) as u64, GICD_INT_EN_CLR_X32);
        wr(&DIST, GIC_DIST_ENABLE_CLEAR + (i / 8) as u64, GICD_INT_EN_CLR_X32);
        i += 32;
    }
    let mut i = 0;
    while i < 32 {
        wr(&DIST, GIC_DIST_PRI + (i * 4 / 4) as u64, repeat_byte(GICD_INT_DEF_PRI));
        i += 4;
    }
    wr(&CPU, GIC_CPU_PRIMASK, GICC_INT_PRI_THRESHOLD);
    let mode = if SPLIT_EOI.load(Ordering::Relaxed) { GIC_CPU_CTRL_EOIMODENS } else { 0 };
    if is_gicv2() {
        for i in 0..4 {
            wr(&CPU, GIC_CPU_ACTIVEPRIO + i * 4, 0);
        }
    }
    let bypass = rd(&CPU, GIC_CPU_CTRL) & GICC_DIS_BYPASS_MASK;
    wr(&CPU, GIC_CPU_CTRL, bypass | mode | GICC_ENABLE);
}

/// Find the GIC in the device tree and bring up its distributor and this
/// core's CPU interface. False without a GICv2.
pub fn init(fdt: &Fdt) -> bool {
    let node = fdt.find_compatible(b"arm,gic-400").or_else(|| fdt.find_compatible(b"arm,cortex-a15-gic"));
    let Some(node) = node else { return false };
    let mut regs = [(0u64, 0u64); 2];
    let mut n = 0;
    fdt.reg(&node, |a, s| {
        if n < 2 { regs[n] = (a, s); }
        n += 1;
    });
    if n < 2 { return false; }
    DIST.store(regs[0].0, Ordering::Relaxed);
    CPU.store(regs[1].0, Ordering::Relaxed);
    // gic_check_eoimode: a kernel at EL2 with the full 8 KiB CPU
    // interface (GICC_DIR in its second page).
    SPLIT_EOI.store(super::exception_level() == 2 && regs[1].1 >= 0x2000, Ordering::Relaxed);
    let ctr = rd(&DIST, GIC_DIST_CTR);
    IRQS.store((((ctr & 0x1f) + 1) * 32).min(MAX_IRQS as u32), Ordering::Relaxed);
    dist_init();
    cpu_init();
    true
}

/// Interrupt lines the distributor implements.
pub fn irqs() -> u32 {
    IRQS.load(Ordering::Relaxed)
}

/// Run `handler` for INTID `irq` and unmask it (gic_unmask_irq). PPIs and
/// SGIs are banked: their enable reaches only the calling core.
pub fn enable(irq: u32, handler: fn()) {
    if irq as usize >= MAX_IRQS { return; }
    HANDLERS[irq as usize].store(handler as usize, Ordering::Release);
    unmask(irq);
}

/// gic_unmask_irq alone, for a banked PPI on a further core.
pub fn unmask(irq: u32) {
    if irq as usize >= MAX_IRQS { return; }
    wr(&DIST, GIC_DIST_ENABLE_SET + (irq / 32) as u64 * 4, 1 << (irq % 32));
}

/// gic_handle_irq: acknowledge and run every pending interrupt.
pub fn handle() {
    let split = SPLIT_EOI.load(Ordering::Relaxed);
    loop {
        let stat = rd(&CPU, GIC_CPU_INTACK);
        let irq = stat & GICC_IAR_INT_ID_MASK;
        if irq >= 1020 { break; }
        if split {
            wr(&CPU, GIC_CPU_EOI, stat);
        }
        // SAFETY: an ISB, as Linux after the priority drop.
        unsafe { core::arch::asm!("isb", options(nomem, nostack, preserves_flags)) };
        let h = HANDLERS[irq as usize].load(Ordering::Acquire);
        if h != 0 {
            // SAFETY: only `enable` stores here, always a `fn()`.
            let f: fn() = unsafe { core::mem::transmute(h) };
            f();
        }
        // gic_eoi_irq: deactivate, with the full IAR value for an SGI.
        wr(&CPU, if split { GIC_CPU_DEACTIVATE } else { GIC_CPU_EOI }, if irq < 16 { stat } else { irq });
    }
}
