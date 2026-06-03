//! ACPI Embedded Controller (EC) — polling reads of EC RAM.
//!
//! Ported from Linux `drivers/acpi/ec.c` (polling path). The EC is the
//! microcontroller that owns battery / thermal / lid state on laptops; its
//! 256-byte RAM is read one byte at a time via the RD_EC command over the
//! ISA ports 0x62 (data) / 0x66 (status+command). On HP (and most x86) the
//! battery lives here as plain EC-RAM fields (remaining/full capacity,
//! status) that the DSDT's `_BST`/`_BIF` read — so once we know the offsets
//! we can read charge directly, no AML interpreter needed.
//!
//! READ-ONLY: we never WR_EC. Writing arbitrary EC RAM can brick thermal /
//! charge control. Reads race-tolerantly share the EC with SMM/firmware.

use crate::serial::{inb, outb};

// Default ISA EC ports (FADT EC_BLK; 0x62/0x66 is universal on x86 laptops).
const EC_DATA: u16 = 0x62;
const EC_SC: u16 = 0x66; // status (read) / command (write)

// Status register bits (ec.c).
const EC_FLAG_OBF: u8 = 0x01; // output buffer full → data ready to read
const EC_FLAG_IBF: u8 = 0x02; // input buffer full → controller still busy

const CMD_READ: u8 = 0x80; // RD_EC

fn udelay(us: u64) {
    let freq = crate::interrupts::tsc_freq();
    if freq == 0 {
        for _ in 0..(us * 100) { core::hint::spin_loop(); }
        return;
    }
    let deadline = crate::interrupts::rdtsc() + freq / 1_000_000 * us;
    while crate::interrupts::rdtsc() < deadline { core::hint::spin_loop(); }
}

/// Wait until the input buffer is clear (controller ready for the next
/// byte). ~10 ms cap. Returns false on timeout.
fn wait_ibf_clear() -> bool {
    for _ in 0..2000 {
        if unsafe { inb(EC_SC) } & EC_FLAG_IBF == 0 { return true; }
        udelay(5);
    }
    false
}

/// Wait until the output buffer is full (a read result is available).
fn wait_obf_set() -> bool {
    for _ in 0..2000 {
        if unsafe { inb(EC_SC) } & EC_FLAG_OBF != 0 { return true; }
        udelay(5);
    }
    false
}

/// Read one byte from EC RAM at `addr`. None on timeout. Polling RD_EC:
/// IBF-clear → cmd 0x80 → IBF-clear → addr → OBF-set → data.
pub fn read(addr: u8) -> Option<u8> {
    if !wait_ibf_clear() { return None; }
    unsafe { outb(EC_SC, CMD_READ); }
    if !wait_ibf_clear() { return None; }
    unsafe { outb(EC_DATA, addr); }
    if !wait_obf_set() { return None; }
    Some(unsafe { inb(EC_DATA) })
}

/// Read a little-endian 16-bit word from EC RAM (addr = low byte).
pub fn read_u16(addr: u8) -> Option<u16> {
    let lo = read(addr)? as u16;
    let hi = read(addr.wrapping_add(1))? as u16;
    Some(lo | (hi << 8))
}
