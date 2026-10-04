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

pub const EMPTY_EVENT: KeyEvent = KeyEvent { key: KeyCode::Char(0), modifiers: Modifiers::NONE };

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

    /// Read current modifier state from keyboard driver atomics.
    pub fn current() -> Self {
        Modifiers {
            shift: crate::keyboard::is_shift_held(),
            ctrl: crate::keyboard::is_ctrl_held(),
            alt: false, // no separate Alt tracking yet (AltGr covers Right Alt)
            alt_gr: crate::keyboard::is_alt_held(), // is_alt_held actually tracks AltGr
            super_key: crate::keyboard::is_super_held(),
        }
    }
}
