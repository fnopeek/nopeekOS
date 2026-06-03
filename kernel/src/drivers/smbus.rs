//! Intel ICH/PCH SMBus host controller (i801) — polling word-data reads.
//!
//! Ported 1:1 from Linux `drivers/i2c/busses/i2c-i801.c` (the no-IRQ
//! `i801_wait_intr` / `i801_simple_transaction` path). Vendor-neutral: the
//! controller is found by PCI class 0x0C05 (serial-bus / SMBus), so any
//! Intel PCH exposing the standard i801 register block works without an ID
//! table. Only the SMBus READ_WORD transaction is implemented — that is all
//! the Smart-Battery client (`battery.rs`) needs.

use spin::Mutex;

use crate::pci;
use crate::serial::{inb, outb};

// SMBus I/O register offsets from the SMBA base (i2c-i801.c).
const SMBHSTSTS: u16 = 0;
const SMBHSTCNT: u16 = 2;
const SMBHSTCMD: u16 = 3;
const SMBHSTADD: u16 = 4;
const SMBHSTDAT0: u16 = 5;
const SMBHSTDAT1: u16 = 6;

// PCI config: host configuration register + I/O BAR (BAR4).
const SMBHSTCFG: u8 = 0x40;
const SMBHSTCFG_HST_EN: u32 = 1 << 0;
const SMBBAR_OFFSET: u8 = 0x20; // BAR4 = 0x10 + 4*4

// SMBHSTCNT transaction protocols / control bits.
const I801_WORD_DATA: u8 = 0x0C;
const SMBHSTCNT_KILL: u8 = 1 << 1;
const SMBHSTCNT_START: u8 = 1 << 6;

// SMBHSTSTS status bits.
const STS_HOST_BUSY: u8 = 1 << 0;
const STS_INTR: u8 = 1 << 1;
const STS_DEV_ERR: u8 = 1 << 2;
const STS_BUS_ERR: u8 = 1 << 3;
const STS_FAILED: u8 = 1 << 4;
const STS_BYTE_DONE: u8 = 1 << 7;
const STATUS_ERROR_FLAGS: u8 = STS_FAILED | STS_BUS_ERR | STS_DEV_ERR;
const STATUS_FLAGS: u8 = STS_BYTE_DONE | STS_INTR | STATUS_ERROR_FLAGS;

// SMBus transfer direction in the address byte.
const SMBUS_READ: u8 = 1;

static SMBA: Mutex<Option<u16>> = Mutex::new(None);

/// Find the i801 SMBus controller, enable it, and cache its I/O base.
/// Logs + returns silently if absent — SMBus is a nice-to-have (battery).
pub fn init() {
    let Some(dev) = pci::find_by_class(0x0C, 0x05) else {
        crate::kprintln!("[npk] smbus: no SMBus controller (class 0C05)");
        return;
    };
    let addr = dev.addr;

    // I/O BAR (BAR4): bit0 = I/O space marker, bit1 reserved — mask both off.
    let base = (pci::read32(addr, SMBBAR_OFFSET) & 0xFFFF_FFFC) as u16;
    if base == 0 {
        crate::kprintln!("[npk] smbus: SMBA BAR unassigned");
        return;
    }

    // Enable the host controller (HST_EN), preserving the rest of the dword.
    let cfg = pci::read32(addr, SMBHSTCFG);
    if cfg & SMBHSTCFG_HST_EN == 0 {
        pci::write32(addr, SMBHSTCFG, cfg | SMBHSTCFG_HST_EN);
    }

    *SMBA.lock() = Some(base);
    crate::kprintln!("[npk] smbus: i801 @ I/O 0x{:04x} (enabled)", base);
}

fn udelay(us: u64) {
    let freq = crate::interrupts::tsc_freq();
    if freq == 0 {
        for _ in 0..(us * 100) { core::hint::spin_loop(); }
        return;
    }
    let deadline = crate::interrupts::rdtsc() + freq / 1_000_000 * us;
    while crate::interrupts::rdtsc() < deadline { core::hint::spin_loop(); }
}

/// Poll SMBHSTSTS until the controller is idle with a result — mirrors
/// `i801_wait_intr`. Returns the latched error flags (0 = success) or None
/// on timeout. A healthy word read completes in a handful of 250 µs polls;
/// we cap at ~30 ms (vs Linux's 200 ms) deliberately — this runs on the
/// bar's fiber, so a wedged bus must not stall the clock for a fifth of a
/// second every poll. A best-effort battery read that times out just leaves
/// the segment stale until the next tick.
fn wait_intr(base: u16) -> Option<u8> {
    for _ in 0..120 {
        udelay(250);
        let status = unsafe { inb(base + SMBHSTSTS) };
        let busy = status & STS_HOST_BUSY;
        let done = status & (STATUS_ERROR_FLAGS | STS_INTR);
        if busy == 0 && done != 0 {
            return Some(status & STATUS_ERROR_FLAGS);
        }
    }
    None
}

/// Read a 16-bit word from an SMBus device (SMBus READ_WORD protocol).
/// `addr` is the 7-bit address, `cmd` the register/command byte. Returns
/// None if the controller is absent, busy-stuck, or the device NAKs.
pub fn read_word(addr: u8, cmd: u8) -> Option<u16> {
    let base = (*SMBA.lock())?;

    // Pre-transaction: bail if the bus is wedged busy, then clear any
    // lingering status flags (i801_check_pre).
    let pre = unsafe { inb(base + SMBHSTSTS) };
    if pre & STS_HOST_BUSY != 0 {
        unsafe { outb(base + SMBHSTCNT, SMBHSTCNT_KILL); }
        wait_intr(base);
        unsafe { outb(base + SMBHSTCNT, 0); }
        if unsafe { inb(base + SMBHSTSTS) } & STS_HOST_BUSY != 0 {
            return None;
        }
    }
    let lingering = pre & STATUS_FLAGS;
    if lingering != 0 {
        unsafe { outb(base + SMBHSTSTS, lingering); }
    }

    // Set up the word-data read and kick it off.
    unsafe {
        outb(base + SMBHSTADD, (addr << 1) | SMBUS_READ);
        outb(base + SMBHSTCMD, cmd);
        outb(base + SMBHSTCNT, I801_WORD_DATA | SMBHSTCNT_START);
    }

    let err = wait_intr(base)?;
    if err & STATUS_ERROR_FLAGS != 0 {
        // Clear the error so the next transaction starts clean.
        unsafe { outb(base + SMBHSTSTS, err); }
        return None;
    }

    let lo = unsafe { inb(base + SMBHSTDAT0) } as u16;
    let hi = unsafe { inb(base + SMBHSTDAT1) } as u16;

    // Clear the completion flags.
    let done = unsafe { inb(base + SMBHSTSTS) } & STATUS_FLAGS;
    if done != 0 {
        unsafe { outb(base + SMBHSTSTS, done); }
    }

    Some(lo | (hi << 8))
}
