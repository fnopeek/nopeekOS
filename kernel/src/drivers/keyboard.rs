//! PS/2 Keyboard Driver
//!
//! IRQ1 handler reads scancodes from port 0x60.
//! Scancode Set 1 with US and DE_CH layouts.
//! USB keyboards work via BIOS legacy PS/2 emulation.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::serial::{inb, outb};

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;

// Lock-free ring buffer for decoded key events (interrupt-safe)
const BUF_SIZE: usize = 64;
static mut KEY_BUF: [u8; BUF_SIZE] = [0; BUF_SIZE];
static BUF_HEAD: AtomicUsize = AtomicUsize::new(0);
static BUF_TAIL: AtomicUsize = AtomicUsize::new(0);

// Modifier state (shared across all keyboard drivers)
static SHIFT: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static ALT_GR: AtomicBool = AtomicBool::new(false);
static SUPER: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK: AtomicBool = AtomicBool::new(false);
static EXTENDED: AtomicBool = AtomicBool::new(false);

// --- Public modifier API (used by shade, any driver can write) ---

/// Set Super/GUI/Meta key state. Called by any keyboard driver.
pub fn set_super(held: bool) { SUPER.store(held, Ordering::Release); }

/// Set Shift key state. Called by any keyboard driver.
pub fn set_shift(held: bool) { SHIFT.store(held, Ordering::Release); }

/// Set Ctrl key state. Called by any keyboard driver.
pub fn set_ctrl(held: bool) { CTRL.store(held, Ordering::Release); }

/// Query modifier state (used by shade::input).
pub fn is_super_held() -> bool { SUPER.load(Ordering::Acquire) }
pub fn is_ctrl_held() -> bool { CTRL.load(Ordering::Acquire) }
pub fn is_shift_held() -> bool { SHIFT.load(Ordering::Acquire) }
pub fn is_alt_held() -> bool { ALT_GR.load(Ordering::Acquire) }

// Special key codes (escape sequences sent as ESC [ X)
const KEY_UP: u8 = 0x80;
const KEY_DOWN: u8 = 0x81;
const KEY_LEFT: u8 = 0x82;
const KEY_RIGHT: u8 = 0x83;
const KEY_HOME: u8 = 0x84;
const KEY_END: u8 = 0x85;
const KEY_PGUP: u8 = 0x86;
const KEY_PGDN: u8 = 0x87;
const KEY_DEL: u8 = 0x88;
const KEY_INSERT: u8 = 0x89;

/// Initialize PS/2 keyboard controller.
/// Safe on systems without PS/2 (returns silently).
pub fn init() {
    unsafe {
        // Check if PS/2 controller exists (0xFF = no controller)
        let status = inb(STATUS_PORT);
        if status == 0xFF {
            return; // No PS/2 controller (USB-only system)
        }

        // Flush any pending data from controller buffer (with timeout)
        for _ in 0..1000 {
            if (inb(STATUS_PORT) & 0x01) == 0 { break; }
            let _ = inb(DATA_PORT);
        }

        // Enable keyboard (send 0xAE to command port)
        outb(STATUS_PORT, 0xAE);

        // Enable scanning (send 0xF4 to data port)
        wait_write();
        outb(DATA_PORT, 0xF4);
    }
}

#[allow(dead_code)]
/// Check if a key is available (IRQ buffer or polled port 0x60).
pub fn has_key() -> bool {
    if BUF_HEAD.load(Ordering::Relaxed) != BUF_TAIL.load(Ordering::Relaxed) {
        return true;
    }
    crate::xhci::is_available()
}

/// Read next raw key byte from buffer. Falls back to xHCI USB keyboard.
/// Prefer `read_event()` for typed KeyEvent with modifiers.
pub fn read_key() -> Option<u8> {
    let head = BUF_HEAD.load(Ordering::Acquire);
    let tail = BUF_TAIL.load(Ordering::Acquire);
    if head != tail {
        // SAFETY: single consumer (main loop), IRQ only writes via push_key
        let key = unsafe { KEY_BUF[tail] };
        BUF_TAIL.store((tail + 1) % BUF_SIZE, Ordering::Release);
        return Some(key);
    }
    // xHCI USB keyboard (real driver, no legacy emulation needed)
    crate::xhci::poll_keyboard()
}

/// Read next key as a typed KeyEvent. Converts ANSI escape sequences
/// (ESC [ A/B/C/D/H/F/2/3/5/6) to KeyCode variants. Captures current
/// modifier state (Shift, Ctrl, Alt, AltGr, Super) in the event.
pub fn read_event() -> Option<crate::input::KeyEvent> {
    use crate::input::{KeyEvent, KeyCode, Modifiers};
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

fn push_key(key: u8) {
    let head = BUF_HEAD.load(Ordering::Relaxed);
    let next = (head + 1) % BUF_SIZE;
    if next != BUF_TAIL.load(Ordering::Relaxed) {
        // SAFETY: single producer (IRQ handler), consumer only reads via read_key
        unsafe { KEY_BUF[head] = key; }
        BUF_HEAD.store(next, Ordering::Release);
    }
    // Drop key if buffer full
}

/// Inject a raw key byte into the keyboard buffer as if it came from hardware.
/// Used by the remote debug shell to drive the focused window from outside.
/// Clients should send ANSI escape sequences for arrow/nav keys (ESC [ A etc).
pub fn inject_byte(byte: u8) {
    push_key(byte);
}

/// Push an arrow key as ANSI escape sequence: ESC [ A/B/C/D
fn push_arrow(code: u8) {
    let ch = match code {
        KEY_UP => b'A',
        KEY_DOWN => b'B',
        KEY_RIGHT => b'C',
        KEY_LEFT => b'D',
        KEY_HOME => b'H',
        KEY_END => b'F',
        KEY_DEL => b'3', // ESC [ 3 ~
        KEY_PGUP => b'5',
        KEY_PGDN => b'6',
        KEY_INSERT => b'2',
        _ => return,
    };
    push_key(0x1B); // ESC
    push_key(b'[');
    push_key(ch);
}

fn wait_write() {
    unsafe {
        for _ in 0..10000 {
            if (inb(STATUS_PORT) & 0x02) == 0 { return; }
        }
    }
}

/// Decode a raw scancode into an ASCII character (handles modifiers + extended).
fn decode_scancode(scancode: u8) -> Option<u8> {
    // Extended prefix: set flag, wait for next scancode
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::Relaxed);
        return None;
    }

    let is_extended = EXTENDED.load(Ordering::Relaxed);
    if is_extended {
        EXTENDED.store(false, Ordering::Relaxed);
    }

    let released = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    // Handle extended scancodes (arrow keys, Home, End, etc.)
    if is_extended {
        // Modifiers: handle BOTH press and release
        match code {
            0x1D => { CTRL.store(!released, Ordering::Relaxed); return None; }   // Right Ctrl
            0x38 => { ALT_GR.store(!released, Ordering::Relaxed); return None; } // AltGr (Right Alt)
            _ => {}
        }
        if released { return None; }
        match code {
            0x48 => { push_arrow(KEY_UP); return None; }
            0x50 => { push_arrow(KEY_DOWN); return None; }
            0x4B => { push_arrow(KEY_LEFT); return None; }
            0x4D => { push_arrow(KEY_RIGHT); return None; }
            0x47 => { push_arrow(KEY_HOME); return None; }
            0x4F => { push_arrow(KEY_END); return None; }
            0x49 => { push_arrow(KEY_PGUP); return None; }
            0x51 => { push_arrow(KEY_PGDN); return None; }
            0x53 => { push_arrow(KEY_DEL); return None; }
            0x52 => { push_arrow(KEY_INSERT); return None; }
            0x5B | 0x5C => { set_super(!released); return None; } // Super/Meta (left/right)
            _ => return None,
        }
    }

    // Normal scancodes — modifiers
    match code {
        0x2A | 0x36 => { SHIFT.store(!released, Ordering::Relaxed); return None; }
        0x1D => { CTRL.store(!released, Ordering::Relaxed); return None; }
        0x3A => {
            if !released {
                let prev = CAPS_LOCK.load(Ordering::Relaxed);
                CAPS_LOCK.store(!prev, Ordering::Relaxed);
            }
            return None;
        }
        _ => {}
    }

    if released { return None; }

    let shift = SHIFT.load(Ordering::Relaxed);
    let ctrl = CTRL.load(Ordering::Relaxed);
    let alt_gr = ALT_GR.load(Ordering::Relaxed);
    let caps = CAPS_LOCK.load(Ordering::Relaxed);

    if ctrl && code == 0x2E { return Some(0x03); } // Ctrl+C

    let layout = crate::config::get("keyboard");
    let is_de = !matches!(layout.as_deref(), Some("us"));

    // AltGr: special characters (de_CH layout)
    if alt_gr && is_de {
        return altgr_char_de(code);
    }

    if is_de {
        scancode_to_char_de(code, shift, caps)
    } else {
        scancode_to_char_us(code, shift, caps)
    }
}

/// IRQ1 handler — called from interrupts.rs.
pub fn irq_handler() {
    let scancode = unsafe { inb(DATA_PORT) };
    if let Some(c) = decode_scancode(scancode) {
        push_key(c);
    }
}

/// Scancode Set 1 → ASCII (US layout)
fn scancode_to_char_us(code: u8, shift: bool, caps: bool) -> Option<u8> {
    #[rustfmt::skip]
    const NORMAL: [u8; 58] = [
        0,   0x1B, b'1', b'2', b'3', b'4', b'5', b'6',  // 0x00-0x07
        b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t', // 0x08-0x0F
        b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',  // 0x10-0x17
        b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',  // 0x18-0x1F
        b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',  // 0x20-0x27
        b'\'',b'`', 0,   b'\\',b'z', b'x', b'c', b'v',   // 0x28-0x2F
        b'b', b'n', b'm', b',', b'.', b'/', 0,   b'*',   // 0x30-0x37
        0,   b' ',                                         // 0x38-0x39
    ];

    #[rustfmt::skip]
    const SHIFTED: [u8; 58] = [
        0,   0x1B, b'!', b'@', b'#', b'$', b'%', b'^',
        b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t',
        b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
        b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',
        b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
        b'"', b'~', 0,   b'|', b'Z', b'X', b'C', b'V',
        b'B', b'N', b'M', b'<', b'>', b'?', 0,   b'*',
        0,   b' ',
    ];

    if code as usize >= NORMAL.len() { return None; }

    let ch = if shift { SHIFTED[code as usize] } else { NORMAL[code as usize] };
    if ch == 0 { return None; }

    if caps && !shift && ch >= b'a' && ch <= b'z' { return Some(ch - 32); }
    if caps && shift && ch >= b'A' && ch <= b'Z' { return Some(ch + 32); }

    Some(ch)
}

/// AltGr characters for Swiss German (de_CH) keyboard layout.
/// PS/2 Scancode Set 1 → ASCII.
fn altgr_char_de(code: u8) -> Option<u8> {
    match code {
        0x03 => Some(b'@'),   // AltGr+2
        0x04 => Some(b'#'),   // AltGr+3
        0x08 => Some(b'|'),   // AltGr+7
        0x0D => Some(b'~'),   // AltGr+^
        0x1A => Some(b'['),   // AltGr+ü
        0x1B => Some(b']'),   // AltGr+¨
        0x28 => Some(b'{'),   // AltGr+ä
        0x2B => Some(b'}'),   // AltGr+$
        0x56 => Some(b'\\'),  // AltGr+<
        _ => None,
    }
}

/// Scancode Set 1 → ASCII (Swiss German / DE_CH layout)
fn scancode_to_char_de(code: u8, shift: bool, caps: bool) -> Option<u8> {
    #[rustfmt::skip]
    const NORMAL: [u8; 58] = [
        0,   0x1B, b'1', b'2', b'3', b'4', b'5', b'6',  // 0x00-0x07
        b'7', b'8', b'9', b'0', b'\'',b'^', 0x08, b'\t', // 0x08-0x0F
        b'q', b'w', b'e', b'r', b't', b'z', b'u', b'i',  // 0x10-0x17  (z/y swapped)
        b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',  // 0x18-0x1F
        b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',  // 0x20-0x27
        b'\'',b'<', 0,   b'$', b'y', b'x', b'c', b'v',   // 0x28-0x2F  (z/y swapped)
        b'b', b'n', b'm', b',', b'.', b'-', 0,   b'*',   // 0x30-0x37
        0,   b' ',                                         // 0x38-0x39
    ];

    #[rustfmt::skip]
    const SHIFTED: [u8; 58] = [
        0,   0x1B, b'+', b'"', b'*', 0,    b'%', b'&',  // Shift+4=ç (non-ASCII→0)
        b'/', b'(', b')', b'=', b'?', b'`', 0x08, b'\t', // Shift+7=/
        b'Q', b'W', b'E', b'R', b'T', b'Z', b'U', b'I',
        b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',
        b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
        b'"', b'>', 0,   b'!', b'Y', b'X', b'C', b'V',
        b'B', b'N', b'M', b';', b':', b'_', 0,   b'*',
        0,   b' ',
    ];

    if code as usize >= NORMAL.len() { return None; }

    let ch = if shift { SHIFTED[code as usize] } else { NORMAL[code as usize] };
    if ch == 0 { return None; }

    if caps && !shift && ch >= b'a' && ch <= b'z' { return Some(ch - 32); }
    if caps && shift && ch >= b'A' && ch <= b'Z' { return Some(ch + 32); }

    Some(ch)
}
