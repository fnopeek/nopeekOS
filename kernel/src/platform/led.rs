//! The board's activity LED as a sign of life before any screen works.
//!
//! The device tree names it as the `ACT` child of a `gpio-leds` node; on
//! the Raspberry Pi 5 it sits on a Broadcom STB GPIO block (Linux
//! `drivers/gpio/gpio-brcmstb.c`, through `gpio_generic` with
//! `brcm,gpio-direct`): per bank of 32 lines a DATA and an IODIR register,
//! IODIR bit 0 = output, read-modify-write on DATA.

use super::dt::Fdt;
use core::sync::atomic::{AtomicU64, AtomicU32, Ordering};

const GPIO_ACTIVE_LOW: u32 = 1;
const BANK_BYTES: u64 = 8 * 4;
const GIO_DATA: u64 = 1 * 4;
const GIO_IODIR: u64 = 2 * 4;

/// The bank's register block (0 = no LED), the line's bit, active low.
static BANK: AtomicU64 = AtomicU64::new(0);
static BIT: AtomicU32 = AtomicU32::new(0);
static LOW: AtomicU32 = AtomicU32::new(0);

fn rmw(off: u64, bit: u32, set: bool) {
    let p = (BANK.load(Ordering::Relaxed) + off) as *mut u32;
    // SAFETY: `init` found a brcmstb GPIO block at BANK, which the identity
    // map covers as Device memory; DATA and IODIR are 32-bit registers.
    unsafe {
        let v = core::ptr::read_volatile(p);
        core::ptr::write_volatile(p, if set { v | 1 << bit } else { v & !(1 << bit) });
    }
}

/// Find the ACT LED and drive it as an output, off. False without one.
pub fn init(fdt: &Fdt) -> bool {
    let Some(leds) = fdt.find_compatible(b"gpio-leds") else { return false };
    let mut act = None;
    fdt.walk(|n| {
        if n.ancestors().last() == Some(&leds.offset) && fdt.prop_has_str(n, b"label", b"ACT") {
            act = Some(*n);
            return false;
        }
        true
    });
    let Some(act) = act else { return false };
    let Some((o, len)) = fdt.prop(&act, b"gpios") else { return false };
    if len < 12 { return false; }
    let mut cell = [0u32; 3];
    for (i, c) in cell.iter_mut().enumerate() {
        *c = fdt.cell_u32(o + 4 * i);
    }
    let [phandle, line, flags] = cell;
    let mut ctrl = None;
    fdt.walk(|n| {
        if fdt.prop_u32(n, b"phandle") == Some(phandle) {
            ctrl = Some(*n);
            return false;
        }
        true
    });
    let Some(ctrl) = ctrl else { return false };
    if !fdt.prop_has_str(&ctrl, b"compatible", b"brcm,brcmstb-gpio") { return false; }
    let Some((base, _)) = fdt.reg0(&ctrl) else { return false };
    BANK.store(base + (line / 32) as u64 * BANK_BYTES, Ordering::Relaxed);
    BIT.store(line % 32, Ordering::Relaxed);
    LOW.store(flags & GPIO_ACTIVE_LOW, Ordering::Relaxed);
    // As gpio_generic's direction_output: the level first, then the pin.
    set(false);
    rmw(GIO_IODIR, line % 32, false);
    true
}

/// Turn the LED on or off; nothing without one.
pub fn set(on: bool) {
    if BANK.load(Ordering::Relaxed) == 0 { return; }
    let level = on ^ (LOW.load(Ordering::Relaxed) != 0);
    rmw(GIO_DATA, BIT.load(Ordering::Relaxed), level);
}

/// Blink forever in groups of `n` short flashes with a pause between: a
/// number to read off the board when nothing else gives output.
pub fn code(n: u32) -> ! {
    loop {
        for _ in 0..n {
            set(true);
            crate::time::delay_ms(200);
            set(false);
            crate::time::delay_ms(300);
        }
        crate::time::delay_ms(3000);
    }
}
