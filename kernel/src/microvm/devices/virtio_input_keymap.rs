//! KeyEvent → Linux evdev key sequence for the guest's virtio-input.
//!
//! Our `KeyEvent` is *logical*: `Char(u8)` is already layout-converted
//! ASCII (Shift baked in by the host keyboard driver), specials arrive
//! as `KeyCode::Enter/Tab/Up/...`, and `Ctrl+letter` arrives as the
//! control byte (0x01..0x1A) with `modifiers.ctrl`. The guest runs the
//! default **US** xkb map, so we translate the *desired character* to
//! the US evdev keycode (+ whether Shift must be held) that produces
//! it, then wrap Ctrl/Alt from the modifier snapshot.
//!
//! Emission follows the proven virtio-input discipline (v0.169.3):
//! every state change is its own `EV_KEY` + `SYN_REPORT` frame, and
//! every press has a matching release (the input core de-dupes a
//! press of an already-down key). Modifiers are pressed before / and
//! released after the main key, each in its own frame, so xkb sees
//! the modifier down when it processes the key.

use crate::input::{KeyCode, KeyEvent};
use super::virtio_input_pci::push_input_event;

const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const SYN_REPORT: u16 = 0;

// Linux input-event-codes.h — only the subset we emit.
const KEY_ESC: u16 = 1;
const KEY_BACKSPACE: u16 = 14;
const KEY_TAB: u16 = 15;
const KEY_ENTER: u16 = 28;
const KEY_LEFTCTRL: u16 = 29;
const KEY_LEFTSHIFT: u16 = 42;
const KEY_LEFTALT: u16 = 56;
const KEY_HOME: u16 = 102;
const KEY_UP: u16 = 103;
const KEY_PAGEUP: u16 = 104;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_END: u16 = 107;
const KEY_DOWN: u16 = 108;
const KEY_PAGEDOWN: u16 = 109;
const KEY_INSERT: u16 = 110;
const KEY_DELETE: u16 = 111;
const KEY_F1: u16 = 59;  // F1..F10 = 59..68
const KEY_F11: u16 = 87;
const KEY_F12: u16 = 88;

/// One state change = one `EV_KEY` frame + its `SYN_REPORT`.
fn frame(code: u16, down: bool) {
    push_input_event(EV_KEY, code, if down { 1 } else { 0 });
    push_input_event(EV_SYN, SYN_REPORT, 0);
}

/// ASCII (0x20..=0x7E) → (US evdev keycode, needs Shift). The Shift
/// here comes from the *character* (US layout), not the host's
/// physical Shift state (already folded into the char upstream).
fn ascii_to_key(c: u8) -> Option<(u16, bool)> {
    // KEY_* values inline — this table is the spec, naming each would
    // just add 50 one-use consts.
    let pair = match c {
        b' ' => (57, false),
        b'a' => (30, false), b'b' => (48, false), b'c' => (46, false),
        b'd' => (32, false), b'e' => (18, false), b'f' => (33, false),
        b'g' => (34, false), b'h' => (35, false), b'i' => (23, false),
        b'j' => (36, false), b'k' => (37, false), b'l' => (38, false),
        b'm' => (50, false), b'n' => (49, false), b'o' => (24, false),
        b'p' => (25, false), b'q' => (16, false), b'r' => (19, false),
        b's' => (31, false), b't' => (20, false), b'u' => (22, false),
        b'v' => (47, false), b'w' => (17, false), b'x' => (45, false),
        b'y' => (21, false), b'z' => (44, false),
        b'A' => (30, true), b'B' => (48, true), b'C' => (46, true),
        b'D' => (32, true), b'E' => (18, true), b'F' => (33, true),
        b'G' => (34, true), b'H' => (35, true), b'I' => (23, true),
        b'J' => (36, true), b'K' => (37, true), b'L' => (38, true),
        b'M' => (50, true), b'N' => (49, true), b'O' => (24, true),
        b'P' => (25, true), b'Q' => (16, true), b'R' => (19, true),
        b'S' => (31, true), b'T' => (20, true), b'U' => (22, true),
        b'V' => (47, true), b'W' => (17, true), b'X' => (45, true),
        b'Y' => (21, true), b'Z' => (44, true),
        b'1' => (2, false),  b'2' => (3, false),  b'3' => (4, false),
        b'4' => (5, false),  b'5' => (6, false),  b'6' => (7, false),
        b'7' => (8, false),  b'8' => (9, false),  b'9' => (10, false),
        b'0' => (11, false),
        b'!' => (2, true),   b'@' => (3, true),   b'#' => (4, true),
        b'$' => (5, true),   b'%' => (6, true),   b'^' => (7, true),
        b'&' => (8, true),   b'*' => (9, true),   b'(' => (10, true),
        b')' => (11, true),
        b'-' => (12, false), b'_' => (12, true),
        b'=' => (13, false), b'+' => (13, true),
        b'[' => (26, false), b'{' => (26, true),
        b']' => (27, false), b'}' => (27, true),
        b'\\' => (43, false), b'|' => (43, true),
        b';' => (39, false), b':' => (39, true),
        b'\'' => (40, false), b'"' => (40, true),
        b'`' => (41, false), b'~' => (41, true),
        b',' => (51, false), b'<' => (51, true),
        b'.' => (52, false), b'>' => (52, true),
        b'/' => (53, false), b'?' => (53, true),
        _ => return None,
    };
    Some(pair)
}

fn special_to_key(kc: KeyCode) -> Option<u16> {
    Some(match kc {
        KeyCode::Enter => KEY_ENTER,
        KeyCode::Backspace => KEY_BACKSPACE,
        KeyCode::Tab => KEY_TAB,
        KeyCode::Escape => KEY_ESC,
        KeyCode::Delete => KEY_DELETE,
        KeyCode::Insert => KEY_INSERT,
        KeyCode::Up => KEY_UP,
        KeyCode::Down => KEY_DOWN,
        KeyCode::Left => KEY_LEFT,
        KeyCode::Right => KEY_RIGHT,
        KeyCode::Home => KEY_HOME,
        KeyCode::End => KEY_END,
        KeyCode::PageUp => KEY_PAGEUP,
        KeyCode::PageDown => KEY_PAGEDOWN,
        KeyCode::F(n) if (1..=10).contains(&n) => KEY_F1 + (n as u16 - 1),
        KeyCode::F(11) => KEY_F11,
        KeyCode::F(12) => KEY_F12,
        KeyCode::Char(_) | KeyCode::F(_) => return None,
    })
}

/// Translate one host `KeyEvent` into the guest evdev frames. Returns
/// false if the key has no mapping (caller drops it). Sends a full
/// press+release for the key (we only get press events from the
/// driver), wrapped by Ctrl/Alt/Shift held/released around it.
pub fn forward_key(ev: &KeyEvent) -> bool {
    let m = &ev.modifiers;
    let (code, shift, ctrl, alt) = match ev.key {
        // Printable: char already encodes Shift; honor only Ctrl/Alt
        // from the snapshot (e.g. Ctrl++ zoom, Alt+d address bar).
        KeyCode::Char(c) if (0x20..0x7F).contains(&c) => {
            let (code, sh) = match ascii_to_key(c) {
                Some(v) => v,
                None => return false,
            };
            (code, sh, m.ctrl, m.alt)
        }
        // Ctrl+letter arrives as the control byte (^A=1..^Z=26).
        KeyCode::Char(c) if (1..=26).contains(&c) => {
            let letter = (c - 1) + b'a';
            let code = match ascii_to_key(letter) {
                Some((code, _)) => code,
                None => return false,
            };
            (code, m.shift, true, m.alt)
        }
        KeyCode::Char(_) => return false,
        other => {
            let code = match special_to_key(other) {
                Some(c) => c,
                None => return false,
            };
            (code, m.shift, m.ctrl, m.alt)
        }
    };

    if ctrl { frame(KEY_LEFTCTRL, true); }
    if alt { frame(KEY_LEFTALT, true); }
    if shift { frame(KEY_LEFTSHIFT, true); }
    frame(code, true);
    frame(code, false);
    if shift { frame(KEY_LEFTSHIFT, false); }
    if alt { frame(KEY_LEFTALT, false); }
    if ctrl { frame(KEY_LEFTCTRL, false); }
    true
}
