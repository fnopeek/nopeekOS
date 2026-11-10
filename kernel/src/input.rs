//! Unified input events — KeyEvent replaces raw u8 bytes throughout the system.
//!
//! All keyboard input (PS/2, USB/xHCI) is converted to KeyEvent at the driver
//! level. Consumers (intent loop, shade compositor, WASM apps) work with
//! KeyEvent instead of raw scancodes or ANSI escape sequences.

/// A single keyboard event with full context.
#[derive(Clone, Copy, Debug)]
pub struct KeyEvent {
    /// Logical key code (always set).
    pub key: KeyCode,
    /// Modifier state at time of keypress.
    pub modifiers: Modifiers,
}

/// Logical key codes — hardware-independent.
///
/// Wire-stable: variant order and field shape are part of the widget ABI
/// (`shade::widgets::abi::Event::Key`). Append-only; never
/// reorder. Mirrored in the SDK at `nopeek_widgets::abi::KeyCode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KeyCode {
    /// Printable ASCII character (already layout-converted).
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Escape,
    Delete,
    Insert,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

/// Modifier key state at time of keypress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub alt_gr: bool,
    pub super_key: bool,
}

/// Empty event for array initialization.
#[allow(dead_code)]
/// The tail of a UTF-8 sequence that has to pass the key path byte by byte.
///
/// A key event carries one byte (`KeyCode::Char(u8)` is ABI), and a Latin-1
/// letter such as U+00FC is two. The first byte goes out at once; the rest
/// waits here and is drained before the next key press. For ASCII the buffer
/// is always empty.
///
/// Final byte of `ESC [ x` for F1; F2..F12 follow in order. A private
/// range: no ANSI sequence ends in a lowercase letter after `ESC [`.
pub(crate) const FKEY_FINAL: u8 = b'a';

/// One type with two instances, one per keyboard driver (PS/2 in
/// `drivers::keyboard`, USB-HID in `drivers::xhci`), so both producers share
/// the same logic.
pub struct Utf8Tail {
    buf: [u8; 3],
    len: u8,
}

impl Utf8Tail {
    pub const fn new() -> Utf8Tail { Utf8Tail { buf: [0; 3], len: 0 } }

    /// Submit a character and get its first byte; the rest waits.
    pub fn split(&mut self, c: char) -> u8 {
        if (c as u32) < 0x80 {
            self.len = 0;
            return c as u8;      // ASCII: single byte
        }
        let mut b = [0u8; 4];
        let enc = c.encode_utf8(&mut b).as_bytes();
        let rest = &enc[1..];
        self.buf[..rest.len()].copy_from_slice(rest);
        self.len = rest.len() as u8;
        enc[0]
    }

    /// The next pending byte. Must be called before the next key press,
    /// otherwise the rest of a character is lost behind it.
    pub fn take(&mut self) -> Option<u8> {
        if self.len == 0 { return None }
        let b = self.buf[0];
        self.buf.rotate_left(1);
        self.len -= 1;
        Some(b)
    }
}

impl KeyEvent {
    /// Create a KeyEvent for a printable character.
    pub const fn char(c: u8, modifiers: Modifiers) -> Self {
        KeyEvent { key: KeyCode::Char(c), modifiers }
    }

    /// Create a KeyEvent for a special key.
    pub const fn special(key: KeyCode, modifiers: Modifiers) -> Self {
        KeyEvent { key, modifiers }
    }

    /// Convert to ASCII byte for backwards compatibility (WASM apps, serial).
    /// Returns None for non-printable keys (arrows, F-keys, etc.).
    pub fn to_ascii(&self) -> Option<u8> {
        match self.key {
            KeyCode::Char(c) => Some(c),
            KeyCode::Enter => Some(b'\r'),
            KeyCode::Backspace => Some(0x08),
            KeyCode::Tab => Some(b'\t'),
            KeyCode::Escape => Some(0x1B),
            KeyCode::Delete => Some(0x7F),
            _ => None,
        }
    }

    /// True if this is a printable character (not a special key).
    pub fn is_printable(&self) -> bool {
        // From 0x80 on it is part of a UTF-8 sequence, so text even if not a
        // character on its own. 0x7F (DEL) is not.
        matches!(self.key, KeyCode::Char(c) if c >= 0x20 && c != 0x7F)
    }

    /// True if Mod (Super) key is held.
    pub fn has_mod(&self) -> bool {
        self.modifiers.super_key
    }
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false, ctrl: false, alt: false, alt_gr: false, super_key: false,
    };

    /// The modifier state the keyboard drivers keep.
    pub fn current() -> Self {
        Modifiers {
            shift: is_shift_held(),
            ctrl: is_ctrl_held(),
            alt: is_alt_held(),
            alt_gr: is_alt_gr_held(),
            super_key: is_super_held(),
        }
    }
}

// === The key queue every keyboard driver feeds (Linux's input core between
// atkbd/usbhid and the readers) ===

use core::sync::atomic::{AtomicBool, Ordering};

const FKEY_FINAL_LAST: u8 = FKEY_FINAL + 11;

// Ring buffer for decoded key bytes. Written by the keyboard drivers (the
// PS/2 ISR among them) and by injection from modules and the terminal, read
// by several loops, so every access holds the lock with interrupts masked
// (the ISR takes it too). A multi-byte sequence goes in
// under one hold, so another producer cannot split it.
// A burst of keys arrives faster than the shell drains one per loop
// iteration; a paste does not go through here (`inject_paste`).
const BUF_SIZE: usize = 512;

struct KeyRing {
    buf: [u8; BUF_SIZE],
    head: usize,
    tail: usize,
}

impl KeyRing {
    fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    fn free(&self) -> usize {
        (self.tail + BUF_SIZE - self.head - 1) % BUF_SIZE
    }

    fn pop(&mut self) -> Option<u8> {
        if self.is_empty() {
            return None;
        }
        let b = self.buf[self.tail];
        self.tail = (self.tail + 1) % BUF_SIZE;
        Some(b)
    }

    /// All of `seq` or nothing: half an escape sequence is worse than none.
    fn push_seq(&mut self, seq: &[u8]) {
        if seq.len() > self.free() {
            return;
        }
        for &b in seq {
            self.buf[self.head] = b;
            self.head = (self.head + 1) % BUF_SIZE;
        }
    }
}

static KEY_RING: spin::Mutex<KeyRing> =
    spin::Mutex::new(KeyRing { buf: [0; BUF_SIZE], head: 0, tail: 0 });

/// Queue a key's bytes, all or none.
pub(crate) fn push_seq(seq: &[u8]) {
    crate::arch::without_interrupts(|| KEY_RING.lock().push_seq(seq));
}

// Modifier state (shared across all keyboard drivers)
pub(crate) static SHIFT: AtomicBool = AtomicBool::new(false);
pub(crate) static CTRL: AtomicBool = AtomicBool::new(false);
pub(crate) static ALT: AtomicBool = AtomicBool::new(false);
pub(crate) static ALT_GR: AtomicBool = AtomicBool::new(false);
pub(crate) static SUPER: AtomicBool = AtomicBool::new(false);

// --- Public modifier API (used by shade, any driver can write) ---

/// Set Super/GUI/Meta key state. Called by any keyboard driver.
pub fn set_super(held: bool) { SUPER.store(held, Ordering::Release); }

/// Set Shift key state. Called by any keyboard driver.
pub fn set_shift(held: bool) { SHIFT.store(held, Ordering::Release); }

/// Set Ctrl key state. Called by any keyboard driver.
pub fn set_ctrl(held: bool) { CTRL.store(held, Ordering::Release); }

/// Set left Alt state. Called by any keyboard driver. AltGr is separate.
pub fn set_alt(held: bool) { ALT.store(held, Ordering::Release); }

/// Query modifier state (used by shade::input).
pub fn is_super_held() -> bool { SUPER.load(Ordering::Acquire) }
pub fn is_ctrl_held() -> bool { CTRL.load(Ordering::Acquire) }
pub fn is_shift_held() -> bool { SHIFT.load(Ordering::Acquire) }
pub fn is_alt_held() -> bool { ALT.load(Ordering::Acquire) }
pub fn is_alt_gr_held() -> bool { ALT_GR.load(Ordering::Acquire) }

/// Next key byte: the ring, then pasted text, then the drivers polled
/// directly (the i8042 when no interrupt owns it, the xHCI keyboard).
/// Prefer `read_event()` for a typed KeyEvent with modifiers.
pub fn read_key() -> Option<u8> {
    if let Some(key) = crate::arch::without_interrupts(|| KEY_RING.lock().pop()) {
        return Some(key);
    }
    if let Some(key) = PASTE.lock().pop_front() {
        return Some(key);
    }
    #[cfg(target_arch = "x86_64")]
    if let Some(c) = crate::keyboard::poll() {
        return Some(c);
    }
    crate::xhci::poll_keyboard()
}

/// Read next key as a typed KeyEvent. Converts ANSI escape sequences
/// (ESC [ A/B/C/D/H/F/2/3/5/6, a..l for F1..F12) to KeyCode variants. Captures current
/// modifier state (Shift, Ctrl, Alt, AltGr, Super) in the event.
pub fn read_event() -> Option<KeyEvent> {
    let byte = read_key()?;
    let mods = Modifiers::current();

    match byte {
        0x1B => {
            // ESC — might be start of ANSI escape sequence (pushed as 3 bytes by push_arrow)
            if let Some(bracket) = read_key() {
                if bracket == b'[' {
                    if let Some(code) = read_key() {
                        let key = match code {
                            b'A' => KeyCode::Up,
                            b'B' => KeyCode::Down,
                            b'C' => KeyCode::Right,
                            b'D' => KeyCode::Left,
                            b'H' => KeyCode::Home,
                            b'F' => KeyCode::End,
                            b'5' => KeyCode::PageUp,
                            b'6' => KeyCode::PageDown,
                            b'3' => KeyCode::Delete,
                            b'2' => KeyCode::Insert,
                            c @ FKEY_FINAL..=FKEY_FINAL_LAST => KeyCode::F(c - FKEY_FINAL + 1),
                            _ => return Some(KeyEvent::special(KeyCode::Escape, mods)),
                        };
                        return Some(KeyEvent::special(key, mods));
                    }
                }
                // ESC + non-bracket: return ESC (bracket byte is lost — rare edge case)
            }
            Some(KeyEvent::special(KeyCode::Escape, mods))
        }
        b'\r' | b'\n' => Some(KeyEvent::special(KeyCode::Enter, mods)),
        0x08 => Some(KeyEvent::special(KeyCode::Backspace, mods)),
        0x7F => Some(KeyEvent::special(KeyCode::Delete, mods)),
        b'\t' => Some(KeyEvent::special(KeyCode::Tab, mods)),
        c if c >= 0x20 && c < 0x7F => Some(KeyEvent::char(c, mods)),
        c => Some(KeyEvent::char(c, mods)), // control chars etc.
    }
}

/// Inject a raw key byte into the key ring as if it came from hardware.
/// Used by the remote debug shell to drive the focused window from outside.
/// Clients should send ANSI escape sequences for arrow/nav keys (ESC [ A etc).
pub fn inject_byte(byte: u8) {
    push_seq(&[byte]);
}

/// Pasted text waiting to be read as typed input, after the keys in the
/// ring. On the heap, not in the IRQ ring, so a paste of any length arrives
/// whole.
static PASTE: spin::Mutex<alloc::collections::VecDeque<u8>> =
    spin::Mutex::new(alloc::collections::VecDeque::new());

/// Queue `bytes` as typed input.
pub fn inject_paste(bytes: &[u8]) {
    PASTE.lock().extend(bytes.iter().copied());
}

/// The configured layout, cached: this is read in interrupt context, where
/// `config::get` (a lock and an allocation) must not run.
static IS_DE_LAYOUT: AtomicBool = AtomicBool::new(true);

/// Re-read the `keyboard` setting. Called after the configuration loads and
/// whenever the setting changes.
pub fn cache_layout() {
    let is_de = !matches!(crate::config::get("keyboard").as_deref(), Some("us"));
    IS_DE_LAYOUT.store(is_de, Ordering::Relaxed);
}

/// Whether keys map through the German (de_CH) layout. IRQ-safe.
pub fn is_de_layout() -> bool {
    IS_DE_LAYOUT.load(Ordering::Relaxed)
}
