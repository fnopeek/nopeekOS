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

/// Whether the console keeps an open input line intact (`set_line_repair`).
static LINE_REPAIR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Keep the line a reader is typing on intact when another writer prints:
/// the line is blanked before that output and drawn again after it. For a
/// console whose screen is the UART and the text framebuffer; the desktop's
/// terminals keep their input line themselves.
pub fn set_line_repair(on: bool) {
    LINE_REPAIR.store(on, Ordering::Relaxed);
}

pub fn line_repair() -> bool {
    LINE_REPAIR.load(Ordering::Relaxed)
}

/// Longest open line kept for redrawing, in bytes.
const LINE_MAX: usize = 1024;

/// The text on the console's current line that is not yet ended by a
/// newline, and who wrote it: a prompt and what was typed after it.
struct OpenLine {
    owner: u64,
    text: String,
    /// Columns the line covered before a carriage return, still on screen.
    covered: usize,
    /// Blanked for another writer's output, not drawn again yet.
    hidden: bool,
}

impl OpenLine {
    const fn new() -> Self {
        OpenLine { owner: 0, text: String::new(), covered: 0, hidden: false }
    }

    /// Follow the owner's own output: what stays on the current line.
    fn track(&mut self, s: &str) {
        for c in s.chars() {
            match c {
                '\n' => { self.text.clear(); self.covered = 0; }
                '\r' => { self.covered = self.covered.max(self.text.chars().count()); self.text.clear(); }
                '\x08' => { self.text.pop(); }
                c if self.text.len() < LINE_MAX => self.text.push(c),
                _ => {}
            }
        }
    }

    fn columns(&self) -> usize {
        self.covered.max(self.text.chars().count())
    }
}

/// Who is writing: the running fiber, or the core outside one. A writer in
/// a `foreign` section counts as someone else.
fn writer() -> u64 {
    if !LINE_REPAIR.load(Ordering::Relaxed) { return 0; }
    let id = crate::smp::fiber::current_stack_lo()
        .unwrap_or(u64::MAX - crate::smp::per_core::current_core_id() as u64);
    if FOREIGN.load(Ordering::Relaxed) == id { id ^ (1 << 63) } else { id }
}

/// The writer whose output in a `foreign` section is not part of its line.
static FOREIGN: AtomicU64 = AtomicU64::new(0);

/// Run `f`, whose output is not part of the caller's open line: the input
/// loop's background polls print on behalf of other parts of the kernel.
pub fn foreign<R>(f: impl FnOnce() -> R) -> R {
    let me = crate::smp::fiber::current_stack_lo()
        .unwrap_or(u64::MAX - crate::smp::per_core::current_core_id() as u64);
    let before = FOREIGN.swap(me, Ordering::Relaxed);
    let r = f();
    FOREIGN.store(before, Ordering::Relaxed);
    r
}

pub struct SerialPort {
    uart: Uart,
    line: OpenLine,
}

impl SerialPort {
    pub const fn new() -> Self {
        SerialPort { uart: Uart::new(), line: OpenLine::new() }
    }

    /// To the UART and the screens only: line repair is not console output
    /// and stays out of the log and the mirrors' counts.
    fn screen(&self, s: &str) {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        if let Some(h) = HOOKS.get() { (h.write)(s); }
    }

    /// Before output from `who`: blank another writer's open line, or draw
    /// the owner's line again on a fresh line after a foreign fragment.
    fn before_output(&mut self, who: u64) {
        if !LINE_REPAIR.load(Ordering::Relaxed) { return; }
        if who != self.line.owner {
            if !self.line.hidden && self.line.columns() > 0 {
                let blank: String = core::iter::repeat_n(' ', self.line.columns()).collect();
                self.screen("\r");
                self.screen(&blank);
                self.screen("\r");
                self.line.hidden = true;
            }
        } else if self.line.hidden {
            // Another writer left its line open: end it, as printk ends a
            // continuation another context interrupts.
            let text = core::mem::take(&mut self.line.text);
            self.screen("\n");
            self.screen(&text);
            self.line.text = text;
            self.line.covered = 0;
            self.line.hidden = false;
        }
    }

    /// After output from `who`: the owner's line tracked, or drawn again
    /// once the foreign output ended its line.
    fn after_output(&mut self, who: u64, s: &str) {
        if !LINE_REPAIR.load(Ordering::Relaxed) { return; }
        if who == self.line.owner || (self.line.columns() == 0 && !self.line.hidden) {
            self.line.owner = who;
            self.line.track(s);
        } else if self.line.hidden && s.ends_with('\n') {
            let text = core::mem::take(&mut self.line.text);
            self.screen(&text);
            self.line.text = text;
            self.line.covered = 0;
            self.line.hidden = false;
        }
    }

    pub fn init(&mut self) {
        self.uart.init();
    }

    pub fn write_byte(&self, byte: u8) {
        self.uart.put(byte);
    }

    /// Write byte with framebuffer echo (for read_line echo on headless systems)
    fn echo_byte(&mut self, byte: u8) {
        let who = writer();
        self.before_output(who);
        self.write_byte(byte);
        if let Some(h) = HOOKS.get() { (h.echo)(byte); }
        let mut b = [0u8; 4];
        self.after_output(who, (byte as char).encode_utf8(&mut b));
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
        let who = writer();
        self.before_output(who);
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        WRITE_GEN.fetch_add(1, Ordering::Relaxed);
        capture_bytes(s);
        if let Some(h) = HOOKS.get() { (h.write)(s); }
        self.after_output(who, s);
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
