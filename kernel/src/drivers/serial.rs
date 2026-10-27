//! Serial Console Driver (COM1)
//!
//! Minimal human interface via serial port.
//! QEMU: -serial stdio

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use alloc::string::String;
use crate::hw::{Port, PortRange};

/// Bumped once per console `write_str`. The line editor snapshots
/// this before its async poll batch and reprints the prompt + input
/// if it changed — so background output (guest logs, net, worker
/// intents) never leaves a corrupted, prompt-less input line.
/// Edge-triggered + general (any output source), not microvm-specific.
static WRITE_GEN: AtomicU64 = AtomicU64::new(0);

/// Snapshot of the console-write generation. A change between two
/// reads means something wrote to the console in between.
pub fn write_gen() -> u64 {
    WRITE_GEN.load(Ordering::Relaxed)
}

// SAFETY: the 16550 register blocks of COM1 and COM2, which only this
// driver touches.
const COM1: PortRange = unsafe { PortRange::new(0x3F8, 8) };
const COM2_THR: Port = unsafe { Port::new(0x2F8) };

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new(COM1));

/// The kernel log: every console line, kept in a ring like Linux's printk
/// buffer. Full, it drops its oldest lines; readers copy, never consume.
struct LogRing {
    buf: alloc::collections::VecDeque<u8>,
    cap: usize,
}

impl LogRing {
    fn push(&mut self, s: &str) {
        let bytes = s.as_bytes();
        let bytes = &bytes[bytes.len().saturating_sub(self.cap)..];
        let over = (self.buf.len() + bytes.len()).saturating_sub(self.cap);
        if over > 0 {
            // Whole lines go, so the oldest kept line starts at its start.
            let cut = self.buf.iter().skip(over).position(|&b| b == b'\n')
                .map_or(self.buf.len(), |i| over + i + 1);
            self.buf.drain(..cut);
        }
        self.buf.extend(bytes);
    }
}

static CAPTURE: Mutex<Option<LogRing>> = Mutex::new(None);

/// While set, console output is not logged (`dmesg` printing the log).
static CAPTURE_PAUSED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Start the kernel log. Its size follows the RAM, 1/4096 of it between
/// 256 KiB and 16 MiB (Linux: `log_buf_len`).
pub fn start_capture() {
    let cap = ((crate::memory::usable_bytes() / 4096) as usize).clamp(256 * 1024, 16 * 1024 * 1024);
    *CAPTURE.lock() = Some(LogRing { buf: alloc::collections::VecDeque::with_capacity(cap), cap });
}

/// A copy of the kernel log; it keeps running.
pub fn capture_snapshot() -> String {
    match *CAPTURE.lock() {
        Some(ref r) => {
            let (a, b) = r.buf.as_slices();
            let mut v = alloc::vec::Vec::with_capacity(a.len() + b.len());
            v.extend_from_slice(a);
            v.extend_from_slice(b);
            String::from_utf8_lossy(&v).into_owned()
        }
        None => String::new(),
    }
}

/// Run `f` with console output kept out of the kernel log.
pub fn without_capture(f: impl FnOnce()) {
    CAPTURE_PAUSED.store(true, Ordering::Relaxed);
    f();
    CAPTURE_PAUSED.store(false, Ordering::Relaxed);
}

/// Whether `kdebug!` lines also go to the screen and serial (config
/// `bootlog verbose`). Without it they only reach the capture buffer, so
/// `dmesg` and `sys/log/boot` keep them.
static VERBOSE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub fn set_verbose(on: bool) {
    VERBOSE.store(on, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

/// Capture-only output.
pub fn capture_fmt(args: fmt::Arguments) {
    let line = alloc::format!("{}", args);
    capture_bytes(&line);
}

/// The quiet half of `kdebug!`: into the capture buffer and to the remote
/// mirror (`debug`), not to screen or serial. Before the heap exists there
/// is no capture buffer and the line is dropped.
pub fn quiet_line(args: fmt::Arguments) {
    if CAPTURE.lock().is_none() { return; }
    let line = alloc::format!("{}", args);
    capture_bytes(&line);
    crate::shade::terminal::stream_push_global(&line);
}

/// Append to the kernel log if it runs (called from write_str).
fn capture_bytes(s: &str) {
    if CAPTURE_PAUSED.load(Ordering::Relaxed) { return; }
    if let Some(ref mut r) = *CAPTURE.lock() {
        r.push(s);
    }
}

pub struct SerialPort {
    regs: PortRange,
    port_exists: bool,
}

impl SerialPort {
    pub const fn new(regs: PortRange) -> Self {
        SerialPort { regs, port_exists: false }
    }

    /// Initialize COM1: 115200 baud, 8N1
    pub fn init(&mut self) {
        let r = self.regs;
        // Check if serial port exists (0xFF = no hardware)
        if r.inb(5) == 0xFF {
            self.port_exists = false;
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
            self.port_exists = false;
            return; // Port defective
        }
        r.outb(4, 0x0F);       // Normal operation
        self.port_exists = true;
    }

    pub fn write_byte(&self, byte: u8) {
        if !self.port_exists { return; }
        // Mirror to COM2 — host pipes this to target/serial.log via
        // -serial file: in build.sh. Written first so the log captures
        // every byte even if COM1's THR-empty poll stalls. Real HW
        // discards writes to an absent COM2 port.
        COM2_THR.outb(byte);
        while (self.regs.inb(5) & 0x20) == 0 {}
        self.regs.outb(0, byte);
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
                return self.regs.inb(0);
            }
            core::hint::spin_loop();
        }
    }

    pub fn has_data(&self) -> bool {
        self.port_exists && (self.regs.inb(5) & 0x01) != 0
    }

    /// Read a single byte directly from the UART receive register.
    /// Unlike read_byte(), this does NOT fall back to the USB keyboard.
    /// Caller must check has_data() first. Use from paths that already
    /// poll the USB keyboard separately (e.g. the intent loop) so the
    /// two input sources don't steal each other's keystrokes.
    pub fn read_serial_raw(&self) -> u8 {
        self.regs.inb(0)
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
        WRITE_GEN.fetch_add(1, Ordering::Relaxed);
        capture_bytes(s);
        crate::framebuffer::write_str(s);
        crate::shade::terminal::write(s);
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

/// A detail line: printed only with `bootlog verbose`, always kept in the
/// capture buffer (`dmesg`, `sys/log/boot`) and sent to the remote mirror.
#[macro_export]
macro_rules! kdebug {
    ($($arg:tt)*) => ({
        if $crate::serial::verbose() {
            $crate::kprintln!($($arg)*);
        } else {
            $crate::serial::quiet_line(format_args!("{}\n", format_args!($($arg)*)));
        }
    });
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn outb(port: u16, val: u8) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, al", in("dx") port, in("al") val); }
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn outw(port: u16, val: u16) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, ax", in("dx") port, in("ax") val); }
}

/// Write `val` to I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn outl(port: u16, val: u32) {
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("out dx, eax", in("dx") port, in("eax") val); }
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in al, dx", in("dx") port, out("al") val); }
    val
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in ax, dx", in("dx") port, out("ax") val); }
    val
}

/// Read I/O port `port`.
///
/// # Safety
/// The port must belong to a device the caller drives; an access to a
/// foreign port can reset or reprogram hardware. Prefer `hw::Port`.
#[inline(always)]
pub(crate) unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    // SAFETY: the caller's contract; `in`/`out` touch no memory.
    unsafe { core::arch::asm!("in eax, dx", in("dx") port, out("eax") val); }
    val
}
