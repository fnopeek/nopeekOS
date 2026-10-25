//! Usages to input — the part of Linux `drivers/hid/hid-input.c`
//! (`hidinput_configure_usage`, `hidinput_hid_event`) the kernel consumes.
//!
//! Linux maps a usage once, at connect time, to an input event code; here
//! the same decision is taken per event, since the kernel has no input
//! layer to register codes with. Keyboard usages stay HID usage IDs: the
//! kernel's keyboard path and layout tables are keyed by them.
//!
//! Mapped: the keyboard page; buttons in a mouse or pointer collection;
//! relative X, Y, wheel and AC Pan; mute and volume on the consumer page.
//! Everything else is reported through [`Frame::unmapped`], so a key the
//! kernel cannot act on yet shows up in the log instead of vanishing.
//!
//! Allocation-free: this runs in the USB interrupt.

use crate::{Event, GD_MOUSE, GD_POINTER, MAIN_ITEM_RELATIVE, UP_BUTTON, UP_CONSUMER,
            UP_GENDESK, UP_KEYBOARD, USAGE, USAGE_PAGE};

const GD_X: u32 = UP_GENDESK | 0x30;
const GD_Y: u32 = UP_GENDESK | 0x31;
const GD_WHEEL: u32 = UP_GENDESK | 0x38;
const CP_AC_PAN: u32 = UP_CONSUMER | 0x238;

/// Keys held at once. Linux has no limit; ten fingers and an n-key
/// rollover keyboard stay well below it, and a key past it is dropped.
pub const KEYS_MAX: usize = 32;
/// Actions one report can carry; more are dropped.
const FRAME_MAX: usize = 8;

/// A consumer key the kernel acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Media {
    Mute,
    VolumeUp,
    VolumeDown,
    BrightnessUp,
    BrightnessDown,
}

/// A short list with a fixed capacity.
#[derive(Clone, Copy, Debug)]
pub struct List<T: Copy + Default, const N: usize> {
    items: [T; N],
    len: usize,
}

impl<T: Copy + Default, const N: usize> Default for List<T, N> {
    fn default() -> Self { List { items: [T::default(); N], len: 0 } }
}

impl<T: Copy + Default, const N: usize> List<T, N> {
    fn push(&mut self, v: T) {
        if self.len < N {
            self.items[self.len] = v;
            self.len += 1;
        }
    }
    fn remove(&mut self, i: usize) {
        self.items.copy_within(i + 1..self.len, i);
        self.len -= 1;
    }
    pub fn as_slice(&self) -> &[T] { &self.items[..self.len] }
    pub fn is_empty(&self) -> bool { self.len == 0 }
}

impl Default for Media {
    fn default() -> Self { Media::Mute }
}

/// What one input report changed, after it is processed (`input_sync`).
#[derive(Clone, Copy, Default, Debug)]
pub struct Frame {
    pub dx: i32,
    pub dy: i32,
    pub wheel: i32,
    pub hwheel: i32,
    /// The pointer buttons changed.
    pub buttons_changed: bool,
    /// The held keys changed.
    pub keys_changed: bool,
    /// Consumer keys pressed in this report.
    pub media: List<Media, FRAME_MAX>,
    /// Usages pressed that nothing here maps.
    pub unmapped: List<u32, FRAME_MAX>,
}

/// The state one HID interface has built up: which keys and buttons are
/// down. The input core in Linux drops a key event that repeats the state
/// it already has; this does the same.
#[derive(Clone, Copy, Default, Debug)]
pub struct Input {
    /// Keyboard usage IDs held, in the order they went down.
    keys: List<u8, KEYS_MAX>,
    /// Pointer buttons, bit `n` = button `n + 1` (left, right, middle, …).
    pub buttons: u8,
    frame: Frame,
}

impl Input {
    /// `hidinput_hid_event` for one usage.
    pub fn event(&mut self, e: &Event) {
        let page = e.usage & USAGE_PAGE;
        let id = e.usage & USAGE;
        let relative = e.flags & MAIN_ITEM_RELATIVE != 0;
        match page {
            UP_KEYBOARD => {
                // Usages 0..3 are "no event" and the three error codes;
                // `hid_keyboard[]` maps them to nothing. Past 255 Linux
                // reports KEY_UNKNOWN, which nothing here consumes.
                if !(4..256).contains(&id) { return; }
                self.key(id as u8, e.value != 0);
            }
            UP_BUTTON => {
                let pointer = matches!(e.application, GD_MOUSE | GD_POINTER)
                    || matches!(e.physical, GD_MOUSE | GD_POINTER);
                let code = id.wrapping_sub(1);
                if !pointer || code >= 8 {
                    if e.value != 0 { self.frame.unmapped.push(e.usage); }
                    return;
                }
                let bit = 1u8 << code;
                let was = self.buttons;
                if e.value != 0 { self.buttons |= bit } else { self.buttons &= !bit }
                if self.buttons != was { self.frame.buttons_changed = true; }
            }
            _ => match e.usage {
                GD_X if relative => self.frame.dx += e.value,
                GD_Y if relative => self.frame.dy += e.value,
                GD_WHEEL if relative => self.frame.wheel += e.value,
                CP_AC_PAN => self.frame.hwheel += e.value,
                _ if page == UP_CONSUMER || page == UP_GENDESK => {
                    // Keys on these pages arrive as 1 when pressed and 0 when
                    // released; only the press acts.
                    if e.value == 0 { return; }
                    match e.usage {
                        u if u == UP_CONSUMER | 0xe2 => self.frame.media.push(Media::Mute),
                        u if u == UP_CONSUMER | 0xe9 => self.frame.media.push(Media::VolumeUp),
                        u if u == UP_CONSUMER | 0xea => self.frame.media.push(Media::VolumeDown),
                        // hid-input: 0x06f KEY_BRIGHTNESSUP, 0x070
                        // KEY_BRIGHTNESSDOWN.
                        u if u == UP_CONSUMER | 0x6f => self.frame.media.push(Media::BrightnessUp),
                        u if u == UP_CONSUMER | 0x70 => self.frame.media.push(Media::BrightnessDown),
                        // Absolute axes (touchscreens, joysticks) are not
                        // keys; they repeat their value on every report.
                        _ if page == UP_GENDESK && id < 0x80 => {}
                        _ => self.frame.unmapped.push(e.usage),
                    }
                }
                _ => {
                    if e.value != 0 && !relative { self.frame.unmapped.push(e.usage); }
                }
            },
        }
    }

    fn key(&mut self, id: u8, down: bool) {
        let held = self.keys.as_slice().iter().position(|&k| k == id);
        match (down, held) {
            (true, None) => { self.keys.push(id); self.frame.keys_changed = true; }
            (false, Some(i)) => { self.keys.remove(i); self.frame.keys_changed = true; }
            _ => {}
        }
    }

    /// Keyboard usage IDs held, in the order they went down.
    pub fn keys(&self) -> &[u8] { self.keys.as_slice() }

    /// The modifier byte of a boot keyboard report: usages 0xE0..0xE7 held.
    pub fn modifiers(&self) -> u8 {
        self.keys().iter()
            .filter(|&&k| (0xe0..=0xe7).contains(&k))
            .fold(0u8, |m, &k| m | 1 << (k - 0xe0))
    }

    /// Keys held that are not modifiers, in the order they went down.
    pub fn non_modifier_keys(&self) -> impl Iterator<Item = u8> + '_ {
        self.keys().iter().copied().filter(|k| !(0xe0..=0xe7).contains(k))
    }

    /// End of a report: what it changed, and a fresh frame for the next.
    pub fn sync(&mut self) -> Frame {
        core::mem::take(&mut self.frame)
    }
}
