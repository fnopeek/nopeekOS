//! The kernel console: the UART (`drivers::uart`), the kernel log, and
//! the screens and keyboard hooked in at boot (`set_hooks`).

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use alloc::string::String;
use crate::drivers::uart::Uart;

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

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort::new());

/// What the console reaches besides the UART: the screens that mirror its
/// output, and the keyboard read alongside it.
pub struct Hooks {
    /// Console output, after the UART.
    pub write: fn(&str),
    /// A byte echoed by the line readers.
    pub echo: fn(u8),
    /// A key, if one is waiting.
    pub key: fn() -> Option<u8>,
    /// A `kdebug!` line, which skips the screens but goes to the remote mirror.
    pub quiet: fn(&str),
}

static HOOKS: spin::Once<Hooks> = spin::Once::new();

/// Hook the screens and the keyboard in, once at boot.
pub fn set_hooks(h: Hooks) {
    HOOKS.call_once(|| h);
}

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
    if let Some(h) = HOOKS.get() { (h.quiet)(&line); }
}

/// Append to the kernel log if it runs (called from write_str).
fn capture_bytes(s: &str) {
    if CAPTURE_PAUSED.load(Ordering::Relaxed) { return; }
    if let Some(ref mut r) = *CAPTURE.lock() {
        r.push(s);
    }
}

pub struct SerialPort {
    uart: Uart,
}

impl SerialPort {
    pub const fn new() -> Self {
        SerialPort { uart: Uart::new() }
    }

    pub fn init(&mut self) {
        self.uart.init();
    }

    pub fn write_byte(&self, byte: u8) {
        self.uart.put(byte);
    }

    /// Write byte with framebuffer echo (for read_line echo on headless systems)
    fn echo_byte(&self, byte: u8) {
        self.write_byte(byte);
        if let Some(h) = HOOKS.get() { (h.echo)(byte); }
    }

    pub fn has_data(&self) -> bool {
        self.uart.has_data()
    }

    /// Read a single byte directly from the UART receive register.
    /// Unlike read_byte(), this does NOT fall back to the USB keyboard.
    /// Caller must check has_data() first. Use from paths that already
    /// poll the USB keyboard separately (e.g. the intent loop) so the
    /// two input sources don't steal each other's keystrokes.
    pub fn read_serial_raw(&self) -> u8 {
        self.uart.get()
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
        if let Some(h) = HOOKS.get() { (h.write)(s); }
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

/// A key from the keyboard hook, if one is waiting; never the UART.
pub fn key() -> Option<u8> {
    HOOKS.get().and_then(|h| (h.key)())
}

/// Blocking read of one byte from the keyboard hook or the UART. The
/// console lock is held only to look at the UART, never across the wait:
/// another core printing meanwhile must not stall, least of all the one
/// that delivers the keys.
pub fn read_byte() -> u8 {
    loop {
        if let Some(key) = HOOKS.get().and_then(|h| (h.key)()) {
            return key;
        }
        {
            let s = SERIAL.lock();
            if s.has_data() {
                return s.read_serial_raw();
            }
        }
        core::hint::spin_loop();
    }
}

fn echo(byte: u8) {
    SERIAL.lock().echo_byte(byte);
}

/// Read a line into `buf`, echoing each character (or `*` when `masked`),
/// up to Enter; returns its length.
fn read_line_echo(buf: &mut [u8], masked: bool) -> usize {
    let mut pos = 0;
    loop {
        match read_byte() {
            b'\r' | b'\n' => {
                echo(b'\r');
                echo(b'\n');
                return pos;
            }
            0x08 | 0x7F => {
                if pos > 0 {
                    pos -= 1;
                    echo(0x08);
                    echo(b' ');
                    echo(0x08);
                }
            }
            byte if (0x20..0x7F).contains(&byte) => {
                if pos < buf.len() {
                    buf[pos] = byte;
                    pos += 1;
                    echo(if masked { b'*' } else { byte });
                }
            }
            _ => {}
        }
    }
}

/// Read a line with echo, returns its length.
pub fn read_line(buf: &mut [u8]) -> usize {
    read_line_echo(buf, false)
}

/// Read a line with masked echo (`*`), returns its length.
pub fn read_line_masked(buf: &mut [u8]) -> usize {
    read_line_echo(buf, true)
}
