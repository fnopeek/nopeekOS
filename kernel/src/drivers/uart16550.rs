//! The PC's 16550 UART on COM1, mirrored to COM2.

use crate::arch::x86_64::{Port, PortRange};

// SAFETY: the 16550 register block of COM1, which only this driver touches.
const COM1: PortRange = unsafe { PortRange::new(0x3F8, 8) };
// SAFETY: COM2's transmit register, which only this driver touches.
const COM2_THR: Port = unsafe { Port::new(0x2F8) };

pub struct Uart {
    present: bool,
}

impl Uart {
    pub const fn new() -> Self {
        Uart { present: false }
    }

    /// Initialize COM1: 115200 baud, 8N1
    pub fn init(&mut self) {
        let r = COM1;
        // Check if serial port exists (0xFF = no hardware)
        if r.inb(5) == 0xFF {
            self.present = false;
            return;
        }
        r.outb(1, 0x00);       // Disable interrupts
        r.outb(3, 0x80);       // Enable DLAB
        r.outb(0, 0x01);       // Divisor low: 115200 baud
        r.outb(1, 0x00);       // Divisor high
        r.outb(3, 0x03);       // 8 bits, no parity, one stop
        r.outb(2, 0xC7);       // Enable FIFO, 14-byte threshold
        r.outb(4, 0x0B);       // IRQs off, RTS/DSR set
        r.outb(4, 0x1E);       // Loopback test
        r.outb(0, 0xAE);
        if r.inb(0) != 0xAE {
            self.present = false;
            return; // Port defective
        }
        r.outb(4, 0x0F);       // Normal operation
        self.present = true;
    }

    pub fn put(&self, byte: u8) {
        if !self.present { return; }
        // Mirror to COM2 — host pipes this to target/serial.log via
        // -serial file: in build.sh. Written first so the log captures
        // every byte even if COM1's THR-empty poll stalls. Real HW
        // discards writes to an absent COM2 port.
        COM2_THR.outb(byte);
        while (COM1.inb(5) & 0x20) == 0 {}
        COM1.outb(0, byte);
    }

    pub fn has_data(&self) -> bool {
        self.present && (COM1.inb(5) & 0x01) != 0
    }

    /// The received byte; the caller checked `has_data`.
    pub fn get(&self) -> u8 {
        COM1.inb(0)
    }
}

/// Write `b` without a lock and with a bounded wait, for the fatal path.
pub fn panic_put(b: u8) {
    for _ in 0..100_000 {
        if COM1.inb(5) & 0x20 != 0 { break; }
        core::hint::spin_loop();
    }
    COM1.outb(0, b);
}
