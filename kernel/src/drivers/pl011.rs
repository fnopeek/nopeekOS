//! The Arm PrimeCell PL011 UART (Linux `drivers/tty/serial/amba-pl011.c`),
//! the console of QEMU's `virt` machine and of the Raspberry Pi 5.
//!
//! The baud rate stays as the firmware set it. A UART the firmware left
//! disabled (QEMU without firmware) is switched on with 8 data bits and
//! the FIFOs, as `pl011_startup` does. Its address comes from the device
//! tree (`set_base`).
//!
//! Output goes through a software transmit buffer once the UART's interrupt
//! is wired (`start_irq`): a write fills the FIFO and leaves the rest to the
//! transmit interrupt, as `pl011_start_tx_pio` and `pl011_tx_chars` do,
//! instead of waiting at the line rate (115200 baud: 11.5 KB/s) on the
//! writing core. A full buffer waits as an unbuffered write would.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use spin::Mutex;

const UARTDR: u64 = 0x00;
const UARTFR: u64 = 0x18;
const FR_RXFE: u32 = 1 << 4;
const FR_TXFF: u32 = 1 << 5;
const UARTLCR_H: u64 = 0x2c;
const UARTIFLS: u64 = 0x34;
const UARTIMSC: u64 = 0x38;
const UARTICR: u64 = 0x44;
const IFLS_RX4_8: u32 = 2 << 3;
const IFLS_TX4_8: u32 = 2;
const IM_TX: u32 = 1 << 5;
const ICR_ALL: u32 = 0x7ff;
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

/// Set once the transmit interrupt is wired and output is buffered.
static IRQ_ON: AtomicBool = AtomicBool::new(false);

const TX_CAP: usize = 64 * 1024;

/// Bytes accepted but not yet in the FIFO. Taken with interrupts masked,
/// as the transmit interrupt takes it too.
struct TxRing {
    buf: [u8; TX_CAP],
    head: usize,
    len: usize,
}

impl TxRing {
    fn push(&mut self, b: u8) {
        self.buf[(self.head + self.len) % TX_CAP] = b;
        self.len += 1;
    }

    /// Move bytes into the FIFO while it has room.
    fn fill_fifo(&mut self) {
        while self.len > 0 && read(UARTFR) & FR_TXFF == 0 {
            write(UARTDR, self.buf[self.head] as u32);
            self.head = (self.head + 1) % TX_CAP;
            self.len -= 1;
        }
    }

    /// Empty the buffer into the FIFO, waiting at most `TX_WAIT` polls for
    /// each byte; false if the FIFO stayed full.
    fn drain(&mut self) -> bool {
        while self.len > 0 {
            if !wait_room() { return false; }
            self.fill_fifo();
        }
        true
    }
}

static TX: Mutex<TxRing> = Mutex::new(TxRing { buf: [0; TX_CAP], head: 0, len: 0 });

/// Wait until the FIFO takes a byte; false (and stuck) after `TX_WAIT` polls.
fn wait_room() -> bool {
    let mut n = 0;
    while read(UARTFR) & FR_TXFF != 0 {
        n += 1;
        if n == TX_WAIT {
            STUCK.store(true, Ordering::Relaxed);
            return false;
        }
        core::hint::spin_loop();
    }
    true
}

/// Buffer the output from here on: the console node's interrupt (a GIC
/// SPI, three cells) drives the transmitter, the FIFO trigger levels are
/// Linux's (half full), only the transmit interrupt is unmasked.
pub fn start_irq(fdt: &crate::platform::dt::Fdt) -> Option<u32> {
    let base = BASE.load(Ordering::Acquire);
    if base == 0 { return None; }
    let node = fdt.stdout()
        .filter(|n| fdt.prop_has_str(n, b"compatible", b"arm,pl011"))
        .or_else(|| fdt.find_compatible(b"arm,pl011"))?;
    if fdt.reg0(&node).map(|(b, _)| b) != Some(base) { return None; }
    let kind = fdt.prop_cell(&node, b"interrupts", 0)?;
    let line = fdt.prop_cell(&node, b"interrupts", 1)?;
    if kind != 0 { return None; }
    let irq = line + 32;
    write(UARTIMSC, 0);
    write(UARTICR, ICR_ALL);
    write(UARTIFLS, IFLS_RX4_8 | IFLS_TX4_8);
    crate::arch::aarch64::gic::set_type(irq, false);
    crate::arch::aarch64::gic::enable(irq, on_irq);
    IRQ_ON.store(true, Ordering::Release);
    Some(irq)
}

/// The transmit interrupt: refill the FIFO; with nothing left, mask the
/// interrupt (`pl011_stop_tx`). A refill above the trigger level clears it.
fn on_irq(_: u32) {
    let mut r = TX.lock();
    r.fill_fifo();
    if r.len == 0 {
        write(UARTIMSC, read(UARTIMSC) & !IM_TX);
    }
}

/// Send everything buffered, waiting for the line: before a reset or
/// power-off, which would drop it.
pub fn flush() {
    if !IRQ_ON.load(Ordering::Acquire) || STUCK.load(Ordering::Relaxed) { return; }
    crate::arch::aarch64::cpu::without_interrupts(|| {
        TX.lock().drain();
    });
}

/// `flush` for a core about to stop: a core stopped while holding the
/// buffer leaves it as it is.
pub fn flush_halting() {
    if !IRQ_ON.load(Ordering::Acquire) || STUCK.load(Ordering::Relaxed) { return; }
    crate::arch::aarch64::cpu::without_interrupts(|| {
        if let Some(mut r) = TX.try_lock() {
            r.drain();
        }
    });
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
        if !IRQ_ON.load(Ordering::Acquire) {
            if wait_room() { write(UARTDR, byte as u32); }
            return;
        }
        crate::arch::aarch64::cpu::without_interrupts(|| {
            let mut r = TX.lock();
            r.fill_fifo();
            if r.len == 0 && read(UARTFR) & FR_TXFF == 0 {
                write(UARTDR, byte as u32);
                return;
            }
            // The buffer holds bytes only while the FIFO is full, so the
            // FIFO falls through its trigger level and interrupts later.
            while r.len == TX_CAP {
                if !wait_room() { return; }
                r.fill_fifo();
            }
            r.push(byte);
            write(UARTIMSC, read(UARTIMSC) | IM_TX);
        });
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
    if IRQ_ON.swap(false, Ordering::AcqRel) {
        if let Some(mut r) = TX.try_lock() {
            r.drain();
        }
    }
    for _ in 0..100_000 {
        if read(UARTFR) & FR_TXFF == 0 { break; }
        core::hint::spin_loop();
    }
    write(UARTDR, b as u32);
}
