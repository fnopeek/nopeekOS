//! The Arm PrimeCell PL011 UART (Linux `drivers/tty/serial/amba-pl011.c`),
//! the console of QEMU's `virt` machine and of the Raspberry Pi 5.
//!
//! The baud rate stays as the firmware set it. A UART the firmware left
//! disabled (QEMU without firmware) is switched on with 8 data bits and
//! the FIFOs, as `pl011_startup` does. Its address comes from the device
//! tree (`set_base`).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const UARTDR: u64 = 0x00;
const UARTFR: u64 = 0x18;
const FR_RXFE: u32 = 1 << 4;
const FR_TXFF: u32 = 1 << 5;
const UARTLCR_H: u64 = 0x2c;
const UARTCR: u64 = 0x30;
const LCRH_FEN: u32 = 1 << 4;
const LCRH_WLEN_8: u32 = 3 << 5;
const CR_UARTEN: u32 = 1 << 0;
const CR_TXE: u32 = 1 << 8;
const CR_RXE: u32 = 1 << 9;

/// The register block, identity-mapped as Device memory; 0 = none.
static BASE: AtomicU64 = AtomicU64::new(0);

/// Set once the transmit FIFO stayed full past `TX_WAIT`: a UART without
/// its clock never drains, and the console must not hang on it.
static STUCK: AtomicBool = AtomicBool::new(false);

/// FIFO polls before a full FIFO counts as stuck; a character at
/// 9600 baud drains in about a millisecond, far below this.
const TX_WAIT: u32 = 1_000_000;

/// Use the PL011 at `base` (the device tree's console).
pub fn set_base(base: u64) {
    BASE.store(base, Ordering::Release);
}

fn reg(off: u64) -> Option<*mut u32> {
    match BASE.load(Ordering::Acquire) {
        0 => None,
        b => Some((b + off) as *mut u32),
    }
}

fn read(off: u64) -> u32 {
    // SAFETY: `set_base` named the PL011's register block, which the
    // identity map covers as Device memory; reading FR or DR has no effect
    // beyond taking a received byte.
    reg(off).map_or(0, |p| unsafe { core::ptr::read_volatile(p) })
}

fn write(off: u64, v: u32) {
    if let Some(p) = reg(off) {
        // SAFETY: as in `read`; a DR write sends one byte.
        unsafe { core::ptr::write_volatile(p, v) };
    }
}

pub struct Uart;

impl Uart {
    pub const fn new() -> Self {
        Uart
    }

    pub fn init(&mut self) {
        if BASE.load(Ordering::Relaxed) == 0 { return; }
        if read(UARTCR) & CR_UARTEN == 0 {
            write(UARTLCR_H, LCRH_WLEN_8 | LCRH_FEN);
            write(UARTCR, CR_UARTEN | CR_TXE | CR_RXE);
        }
    }

    pub fn put(&self, byte: u8) {
        if BASE.load(Ordering::Relaxed) == 0 || STUCK.load(Ordering::Relaxed) { return; }
        let mut n = 0;
        while read(UARTFR) & FR_TXFF != 0 {
            n += 1;
            if n == TX_WAIT {
                STUCK.store(true, Ordering::Relaxed);
                return;
            }
            core::hint::spin_loop();
        }
        write(UARTDR, byte as u32);
    }

    pub fn has_data(&self) -> bool {
        BASE.load(Ordering::Relaxed) != 0 && read(UARTFR) & FR_RXFE == 0
    }

    /// The received byte; the caller checked `has_data`.
    pub fn get(&self) -> u8 {
        read(UARTDR) as u8
    }
}

/// Write `b` without a lock and with a bounded wait, for the fatal path.
pub fn panic_put(b: u8) {
    for _ in 0..100_000 {
        if read(UARTFR) & FR_TXFF == 0 { break; }
        core::hint::spin_loop();
    }
    write(UARTDR, b as u32);
}
