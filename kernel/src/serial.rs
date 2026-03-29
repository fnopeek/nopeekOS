//! Serial Console Driver (COM1)
//!
//! Minimal human interface via serial port.
//! QEMU: -serial stdio

use core::fmt;
use spin::Mutex;

const COM1: u16 = 0x3F8;

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        SerialPort { base }
    }

    /// Initialize COM1: 115200 baud, 8N1
    pub fn init(&self) {
        unsafe {
            outb(self.base + 1, 0x00);       // Disable interrupts
            outb(self.base + 3, 0x80);       // Enable DLAB
            outb(self.base + 0, 0x01);       // Divisor low: 115200 baud
            outb(self.base + 1, 0x00);       // Divisor high
            outb(self.base + 3, 0x03);       // 8 bits, no parity, one stop
            outb(self.base + 2, 0xC7);       // Enable FIFO, 14-byte threshold
            outb(self.base + 4, 0x0B);       // IRQs off, RTS/DSR set
            outb(self.base + 4, 0x1E);       // Loopback test
            outb(self.base + 0, 0xAE);
            if inb(self.base + 0) != 0xAE {
                return; // Port defective
            }
            outb(self.base + 4, 0x0F);       // Normal operation
        }
    }

    pub fn write_byte(&self, byte: u8) {
        unsafe {
            while (inb(self.base + 5) & 0x20) == 0 {}
            outb(self.base, byte);
        }
    }

    /// Blocking read. Uses hlt to sleep until timer interrupt wakes us.
    pub fn read_byte(&self) -> u8 {
        unsafe {
            while (inb(self.base + 5) & 0x01) == 0 {
                // SAFETY: hlt stops CPU until next interrupt (requires sti)
                core::arch::asm!("hlt");
            }
            inb(self.base)
        }
    }

    pub fn has_data(&self) -> bool {
        unsafe { (inb(self.base + 5) & 0x01) != 0 }
    }

    /// Read a line with echo, returns length
    pub fn read_line(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        loop {
            let byte = self.read_byte();
            match byte {
                b'\r' | b'\n' => {
                    self.write_byte(b'\r');
                    self.write_byte(b'\n');
                    return pos;
                }
                0x08 | 0x7F => {
                    if pos > 0 {
                        pos -= 1;
                        self.write_byte(0x08);
                        self.write_byte(b' ');
                        self.write_byte(0x08);
                    }
                }
                byte if byte >= 0x20 && byte < 0x7F => {
                    if pos < buf.len() {
                        buf[pos] = byte;
                        pos += 1;
                        self.write_byte(byte);
                    }
                }
                _ => {}
            }
        }
    }
}

impl fmt::Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        let mut serial = $crate::serial::SERIAL.lock();
        write!(serial, $($arg)*).unwrap();
    });
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::kprint!("{}\n", format_args!($($arg)*)));
}

#[inline(always)]
pub(crate) unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al", in("dx") port, in("al") val);
}

#[inline(always)]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val);
    val
}
