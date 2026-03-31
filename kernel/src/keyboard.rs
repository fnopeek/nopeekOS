//! PS/2 Keyboard Driver
//!
//! IRQ1 handler reads scancodes from port 0x60.
//! Scancode Set 1 with US and DE_CH layouts.
//! USB keyboards work via BIOS legacy PS/2 emulation.

use core::sync::atomic::{AtomicBool, Ordering};
use crate::serial::{inb, outb};

const DATA_PORT: u16 = 0x60;
const STATUS_PORT: u16 = 0x64;

// Ring buffer for decoded key events
const BUF_SIZE: usize = 64;
static mut KEY_BUF: [u8; BUF_SIZE] = [0; BUF_SIZE];
static mut BUF_HEAD: usize = 0;
static mut BUF_TAIL: usize = 0;

// Modifier state
static SHIFT: AtomicBool = AtomicBool::new(false);
static CTRL: AtomicBool = AtomicBool::new(false);
static CAPS_LOCK: AtomicBool = AtomicBool::new(false);

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

/// Check if a key is available in the buffer.
pub fn has_key() -> bool {
    // SAFETY: single-core, IRQ handler is the only writer
    unsafe { BUF_HEAD != BUF_TAIL }
}

/// Read next key from buffer. Returns None if empty.
pub fn read_key() -> Option<u8> {
    unsafe {
        if BUF_HEAD == BUF_TAIL {
            return None;
        }
        let key = KEY_BUF[BUF_TAIL];
        BUF_TAIL = (BUF_TAIL + 1) % BUF_SIZE;
        Some(key)
    }
}

fn push_key(key: u8) {
    unsafe {
        let next = (BUF_HEAD + 1) % BUF_SIZE;
        if next != BUF_TAIL {
            KEY_BUF[BUF_HEAD] = key;
            BUF_HEAD = next;
        }
        // Drop key if buffer full
    }
}

fn wait_write() {
    unsafe {
        for _ in 0..10000 {
            if (inb(STATUS_PORT) & 0x02) == 0 { return; }
        }
    }
}

/// IRQ1 handler — called from interrupts.rs.
pub fn irq_handler() {
    // SAFETY: called from interrupt context, port 0x60 has the scancode
    let scancode = unsafe { inb(DATA_PORT) };

    // Extended scancode prefix (0xE0) — skip for now
    if scancode == 0xE0 {
        return;
    }

    let released = scancode & 0x80 != 0;
    let code = scancode & 0x7F;

    // Handle modifier keys
    match code {
        0x2A | 0x36 => { // Left/Right Shift
            SHIFT.store(!released, Ordering::Relaxed);
            return;
        }
        0x1D => { // Left Ctrl
            CTRL.store(!released, Ordering::Relaxed);
            return;
        }
        0x3A => { // Caps Lock (toggle on press)
            if !released {
                let prev = CAPS_LOCK.load(Ordering::Relaxed);
                CAPS_LOCK.store(!prev, Ordering::Relaxed);
            }
            return;
        }
        _ => {}
    }

    // Only process key presses, not releases
    if released { return; }

    let shift = SHIFT.load(Ordering::Relaxed);
    let ctrl = CTRL.load(Ordering::Relaxed);
    let caps = CAPS_LOCK.load(Ordering::Relaxed);

    // Ctrl+C → 0x03 (ETX)
    if ctrl && code == 0x2E {
        push_key(0x03);
        return;
    }

    // Get layout based on config
    let layout = crate::config::get("keyboard");
    let ch = match layout.as_deref() {
        Some("de_CH") | Some("de") | Some("de_DE") => scancode_to_char_de(code, shift, caps),
        _ => scancode_to_char_us(code, shift, caps),
    };

    if let Some(c) = ch {
        push_key(c);
    }
}

/// Scancode Set 1 → ASCII (US layout)
fn scancode_to_char_us(code: u8, shift: bool, caps: bool) -> Option<u8> {
    // Base (unshifted) mapping for Scancode Set 1
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

    let ch = if shift {
        SHIFTED[code as usize]
    } else {
        NORMAL[code as usize]
    };

    if ch == 0 { return None; }

    // Caps lock: toggle case for letters only
    if caps && !shift && ch >= b'a' && ch <= b'z' {
        return Some(ch - 32);
    }
    if caps && shift && ch >= b'A' && ch <= b'Z' {
        return Some(ch + 32);
    }

    Some(ch)
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
        0,   0x1B, b'+', b'"', b'*', b'/', b'%', b'&',
        b'|', b'(', b')', b'=', b'?', b'`', 0x08, b'\t',
        b'Q', b'W', b'E', b'R', b'T', b'Z', b'U', b'I',
        b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',
        b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
        b'"', b'>', 0,   b'!', b'Y', b'X', b'C', b'V',
        b'B', b'N', b'M', b';', b':', b'_', 0,   b'*',
        0,   b' ',
    ];

    if code as usize >= NORMAL.len() { return None; }

    let ch = if shift {
        SHIFTED[code as usize]
    } else {
        NORMAL[code as usize]
    };

    if ch == 0 { return None; }

    if caps && !shift && ch >= b'a' && ch <= b'z' {
        return Some(ch - 32);
    }
    if caps && shift && ch >= b'A' && ch <= b'Z' {
        return Some(ch + 32);
    }

    Some(ch)
}
