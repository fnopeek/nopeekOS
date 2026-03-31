//! Serial Console Driver (COM1)
//!
//! Minimal human interface via serial port.
//! QEMU: -serial stdio

use core::fmt;
use spin::Mutex;
use alloc::string::String;

const COM1: u16 = 0x3F8;

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

// Output capture for npk-shell: when active, all kprint output is also buffered
static CAPTURE: Mutex<Option<String>> = Mutex::new(None);

/// Start capturing serial output into a buffer.
pub fn start_capture() {
    *CAPTURE.lock() = Some(String::new());
}

/// Stop capturing and return the captured output.
pub fn stop_capture() -> String {
    CAPTURE.lock().take().unwrap_or_default()
}

/// Append to capture buffer if active (called from write_str).
fn capture_bytes(s: &str) {
    if let Some(ref mut buf) = *CAPTURE.lock() {
        buf.push_str(s);
    }
}

pub struct SerialPort {
    base: u16,
    port_exists: bool,
}

impl SerialPort {
    pub const fn new(base: u16) -> Self {
        SerialPort { base, port_exists: false }
    }

    /// Initialize COM1: 115200 baud, 8N1
    pub fn init(&mut self) {
        unsafe {
            // Check if serial port exists (0xFF = no hardware)
            if inb(self.base + 5) == 0xFF {
                self.port_exists = false;
                return;
            }
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
                self.port_exists = false;
                return; // Port defective
            }
            outb(self.base + 4, 0x0F);       // Normal operation
            self.port_exists = true;
        }
    }

    pub fn write_byte(&self, byte: u8) {
        if !self.port_exists { return; }
        unsafe {
            while (inb(self.base + 5) & 0x20) == 0 {}
            outb(self.base, byte);
        }
    }

    /// Write byte with framebuffer echo (for read_line echo on headless systems)
    fn echo_byte(&self, byte: u8) {
        self.write_byte(byte);
        crate::framebuffer::write_byte(byte);
    }

    /// Blocking read. Polls both serial port and USB keyboard.
    pub fn read_byte(&self) -> u8 {
        loop {
            // Check keyboard first (USB/xHCI — primary on bare metal)
            if let Some(key) = crate::keyboard::read_key() {
                return key;
            }
            // Check serial (only if port exists — 0xFF means no hardware)
            if self.port_exists && self.has_data() {
                return unsafe { inb(self.base) };
            }
            core::hint::spin_loop();
        }
    }

    pub fn has_data(&self) -> bool {
        self.port_exists && unsafe { (inb(self.base + 5) & 0x01) != 0 }
    }

    /// Read a line with masked echo (shows '*'), returns length.
    pub fn read_line_masked(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        loop {
            let byte = self.read_byte();
            match byte {
                b'\r' | b'\n' => {
                    self.echo_byte(b'\r');
                    self.echo_byte(b'\n');
                    return pos;
                }
                0x08 | 0x7F => {
                    if pos > 0 {
                        pos -= 1;
                        self.echo_byte(0x08);
                        self.echo_byte(b' ');
                        self.echo_byte(0x08);
                    }
                }
                byte if byte >= 0x20 && byte < 0x7F => {
                    if pos < buf.len() {
                        buf[pos] = byte;
                        pos += 1;
                        self.echo_byte(b'*');
                    }
                }
                _ => {}
            }
        }
    }

    /// Read a line with echo, returns length
    pub fn read_line(&self, buf: &mut [u8]) -> usize {
        let mut pos = 0;
        loop {
            let byte = self.read_byte();
            match byte {
                b'\r' | b'\n' => {
                    self.echo_byte(b'\r');
                    self.echo_byte(b'\n');
                    return pos;
                }
                0x08 | 0x7F => {
                    if pos > 0 {
                        pos -= 1;
                        self.echo_byte(0x08);
                        self.echo_byte(b' ');
                        self.echo_byte(0x08);
                    }
                }
                byte if byte >= 0x20 && byte < 0x7F => {
                    if pos < buf.len() {
                        buf[pos] = byte;
                        pos += 1;
                        self.echo_byte(byte);
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
        capture_bytes(s);
        crate::framebuffer::write_str(s);
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
pub(crate) unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax", in("dx") port, in("ax") val);
}

#[inline(always)]
pub(crate) unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val);
}

#[inline(always)]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!("in al, dx", in("dx") port, out("al") val);
    val
}

#[inline(always)]
pub(crate) unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    core::arch::asm!("in ax, dx", in("dx") port, out("ax") val);
    val
}

#[inline(always)]
pub(crate) unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!("in eax, dx", in("dx") port, out("eax") val);
    val
}
