//! PSCI firmware calls (Arm DEN 0022, Linux `drivers/firmware/psci/psci.c`):
//! the conduit from the device tree, CPU_ON for `cores`, and the system
//! reset and power-off that Linux registers as restart handler and
//! `pm_power_off`.
//!
//! The `psci` node names the conduit: `smc` on the Pi 5 (Arm Trusted
//! Firmware at EL3), `smc` or `hvc` on QEMU.

use crate::platform::dt::Fdt;
use core::sync::atomic::{AtomicU8, Ordering};

pub const PSCI_0_2_FN64_CPU_ON: u64 = 0xC400_0003;
const PSCI_0_2_FN_SYSTEM_OFF: u64 = 0x8400_0008;
const PSCI_0_2_FN_SYSTEM_RESET: u64 = 0x8400_0009;
pub const PSCI_RET_SUCCESS: i64 = 0;
pub const PSCI_RET_ALREADY_ON: i64 = -4;

#[derive(Clone, Copy, PartialEq)]
enum Conduit {
    None = 0,
    Smc = 1,
    Hvc = 2,
}

static CONDUIT: AtomicU8 = AtomicU8::new(Conduit::None as u8);

fn conduit() -> Conduit {
    match CONDUIT.load(Ordering::Acquire) {
        1 => Conduit::Smc,
        2 => Conduit::Hvc,
        _ => Conduit::None,
    }
}

/// `psci_probe`'s conduit from the `psci` node; whether there is one.
pub fn init(fdt: &Fdt) -> bool {
    let c = fdt.find_compatible(b"arm,psci-0.2").or_else(|| fdt.find_compatible(b"arm,psci-1.0"))
        .map_or(Conduit::None, |node| {
            if fdt.prop_has_str(&node, b"method", b"smc") {
                Conduit::Smc
            } else if fdt.prop_has_str(&node, b"method", b"hvc") {
                Conduit::Hvc
            } else {
                Conduit::None
            }
        });
    CONDUIT.store(c as u8, Ordering::Release);
    c != Conduit::None
}

/// `invoke_psci_fn`: an SMCCC call through the conduit; without one,
/// PSCI_RET_NOT_SUPPORTED.
pub fn call(f: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let r: u64;
    // SAFETY: an SMCCC call to the PSCI firmware; x0-x3 are its arguments,
    // x0 its result, and it may clobber x0-x17.
    unsafe {
        match conduit() {
            Conduit::Smc => core::arch::asm!("smc #0",
                inlateout("x0") f => r, inlateout("x1") a1 => _, inlateout("x2") a2 => _, inlateout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _,
                out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _,
                options(nostack)),
            Conduit::Hvc => core::arch::asm!("hvc #0",
                inlateout("x0") f => r, inlateout("x1") a1 => _, inlateout("x2") a2 => _, inlateout("x3") a3 => _,
                out("x4") _, out("x5") _, out("x6") _, out("x7") _, out("x8") _, out("x9") _, out("x10") _,
                out("x11") _, out("x12") _, out("x13") _, out("x14") _, out("x15") _, out("x16") _, out("x17") _,
                options(nostack)),
            Conduit::None => return -1,
        }
    }
    r as i64
}

/// `psci_sys_reset` for a cold reboot (`reboot_mode` REBOOT_COLD, so
/// SYSTEM_RESET, not SYSTEM_RESET2). Returns only if the firmware refused.
pub fn system_reset() {
    call(PSCI_0_2_FN_SYSTEM_RESET, 0, 0, 0);
}

/// `psci_sys_poweroff`. Returns only if the firmware refused.
pub fn system_off() {
    call(PSCI_0_2_FN_SYSTEM_OFF, 0, 0, 0);
}
